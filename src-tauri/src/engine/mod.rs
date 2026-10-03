//! Columnar query engine for indexed sources, backed by an embedded DuckDB.
//!
//! Each file part gets a cached store holding, per line, the values the app
//! computes when it parses the line (`event_at`). Filters, counts, groupings,
//! histograms and charts then run as SQL over those columns, calling the
//! app's own Rust comparisons for every value test. Whatever the store cannot
//! answer exactly is confirmed on the parsed events or answered by the line
//! engine, which also remains the fallback for any engine error.
mod build;
mod ops;
mod sparse_timeline;
mod sql;
mod text;
mod time_index;
mod udf;

pub(crate) use ops::*;

use crate::model::CodesConfig;
use crate::sources::{CompiledDerived, FileIndex};
use duckdb::Connection;
use parking_lot::{Mutex, RwLock};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

static ENABLED: AtomicBool = AtomicBool::new(!cfg!(test));
static CATALOG_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Turns the engine on or off (tests compare both engines).
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::SeqCst);
}
fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst) && std::env::var_os("LOGINSIGHT_ENGINE").is_none_or(|v| v != "0")
}

/// Names and descriptions changed; they are joined at query time.
pub(crate) fn catalogs_changed() {
    CATALOG_EPOCH.fetch_add(1, Ordering::SeqCst);
}

pub(crate) fn engine_dir() -> PathBuf {
    std::env::var_os("LOGINSIGHT_ENGINE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config_dir().join("engine-v1"))
}

/// Derived fields: when present, names and descriptions are resolved while
/// building (rules may test them), so the catalogs join the store key.
fn derived_signature(derived: &[CompiledDerived]) -> Option<String> {
    if derived.is_empty() { return Some(String::new()); }
    let mut definitions = Vec::with_capacity(derived.len());
    let mut lookups = Vec::new();
    for field in derived {
        if field.source == "id" { return None; }
        if let Some(lookup) = &field.lookup {
            if lookup.definition.keys.iter().any(|key| key.source_field == "id") { return None; }
            lookups.push((&field.name, &lookup.definition, lookup.version()?));
        }
        let mut rules = Vec::with_capacity(field.rules.len());
        for rule in &field.rules {
            if rule.filter_reads_id() { return None; }
            // The same textual rule ID can compile differently in two Cases.
            // Use the signature captured with its immutable predicate, never a
            // later TLS/default catalog (including background store builds).
            rules.push((rule.re.as_str(), &rule.template, &rule.filter, &rule.filter_security_signature));
        }
        definitions.push((&field.name, &field.source, &field.steps, rules));
    }
    // Version all derived overlays (including legacy regex) because bounded
    // extraction/provenance semantics changed. Base immutable stores stay valid.
    if lookups.is_empty() {
        return Some(format!("derived-overlay-v3:{}:{}", crate::field_transform::VERSION, serde_json::to_string(&definitions).ok()?));
    }
    Some(format!("derived-overlay-v4:{}:{}:{}", crate::field_transform::VERSION, crate::reference_lookup::VERSION, serde_json::to_string(&(definitions, lookups)).ok()?))
}

fn catalogs_signature(codes: &CodesConfig, system: &CodesConfig) -> String {
    // Request-local Case snapshots can reuse the same stack address and entry
    // count. Only their actual content proves enrichment identity; a pointer
    // cache can return another Case's signature even without an epoch change.
    format!("catalog-v2:{}", catalog_content_signature(codes, system))
}

type CatalogKey = (u64, String);
fn catalog_key(codes: &CodesConfig, system: &CodesConfig) -> CatalogKey {
    // Reader threads share source metadata but own catalog snapshots. Their
    // address must not invalidate a completed selection on every iterator pass.
    let mut hash = Sha256::new();
    hash.update(b"loginsight-catalog-content-v2\0");
    for catalog in [codes, system] {
        hash.update((catalog.sources.values().map(|entries| entries.len()).sum::<usize>() as u64).to_le_bytes());
        let mut sources: Vec<_> = catalog.sources.iter().collect();
        sources.sort_by(|a, b| a.0.cmp(b.0));
        for (source, entries) in sources {
            let mut entries: Vec<_> = entries.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (code, info) in entries {
                for value in [source.as_str(), code.as_str(), info.name.as_str(), info.description.as_str()] {
                    hash.update((value.len() as u64).to_le_bytes());
                    hash.update(value.as_bytes());
                }
            }
        }
        hash.update([255]);
    }
    (CATALOG_EPOCH.load(Ordering::SeqCst), format!("{:x}", hash.finalize()))
}

/// Content identity for projected-row admission. Length-prefixed values and
/// explicit catalog boundaries make control characters unambiguous. Compute
/// under the catalogs' read guards; this proves
/// the actual content even before a mutation increments CATALOG_EPOCH.
pub(crate) fn catalog_content_signature(codes: &CodesConfig, system: &CodesConfig) -> String {
    catalog_key(codes, system).1
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CatalogToken {
    pub signature: String,
    pub epoch: u64,
}

/// Capture under both catalog read guards. Comparing before/after admission
/// detects intervening edits even if the catalog content returns to its start.
pub(crate) fn catalog_token(codes: &CodesConfig, system: &CodesConfig) -> CatalogToken {
    let (epoch, signature) = catalog_key(codes, system);
    CatalogToken { signature, epoch }
}

#[cfg(test)]
mod catalog_identity_tests {
    use super::*;
    use crate::model::CodeInfo;

    fn catalog(name: &str, description: &str) -> CodesConfig {
        let mut config = CodesConfig::default();
        config.sources.entry("source".into()).or_default().insert("1".into(), CodeInfo { name: name.into(), description: description.into() });
        config
    }

    #[test]
    fn persisted_catalog_signature_is_unambiguous_and_epoch_aware() {
        let mut first = catalog("name\u{1}part", "description");
        let second = catalog("name", "part\u{1}description");
        let empty = CodesConfig::default();
        let first_sig = catalogs_signature(&first, &empty);
        let second_sig = catalogs_signature(&second, &empty);
        assert!(first_sig.starts_with("catalog-v2:"));
        assert_ne!(first_sig, second_sig, "separator data must not alias different enrichment");
        assert_eq!(first_sig, format!("catalog-v2:{}", catalog_content_signature(&first, &empty)));
        let _ = catalogs_signature(&first, &empty); // exercise the same address/count before mutation
        first.sources.get_mut("source").unwrap().get_mut("1").unwrap().description = "changed".into();
        catalogs_changed();
        assert_ne!(first_sig, catalogs_signature(&first, &empty));
        assert_ne!(catalogs_signature(&first, &empty), catalogs_signature(&empty, &first));
    }

    #[test]
    fn request_local_catalog_identity_cannot_alias_same_address_and_count() {
        let mut slot = catalog("Case A", "first interpretation");
        let empty = CodesConfig::default();
        let before_epoch = CATALOG_EPOCH.load(Ordering::SeqCst);
        let first = catalogs_signature(&slot, &empty);
        let same_address = &slot as *const _ as usize;
        slot.sources.get_mut("source").unwrap().get_mut("1").unwrap().name = "Case B".into();
        assert_eq!(&slot as *const _ as usize, same_address);
        assert_eq!(CATALOG_EPOCH.load(Ordering::SeqCst), before_epoch);
        let second = catalogs_signature(&slot, &empty);
        assert_ne!(first, second, "Case snapshots must use content identity without relying on a global mutation notification");
        slot.sources.get_mut("source").unwrap().get_mut("1").unwrap().name = "Case A".into();
        assert_eq!(first, catalogs_signature(&slot, &empty));
    }

    #[test]
    fn names_guard_repins_requested_case_after_interleaved_session_acquisition() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE enr(source VARCHAR, code VARCHAR, name VARCHAR, description VARCHAR, catalog INTEGER)").unwrap();
        let session = Arc::new(Session {
            key: "catalog-interleaving".into(), base: Mutex::new(conn), pool: Mutex::new(vec![]),
            schema: sql::Schema::default(), timestamps_non_null: false, baked: false,
            names: RwLock::new(None), names_version: AtomicU64::new(0),
            selections: Mutex::new(vec![]), selection_builds: Mutex::new(HashSet::new()),
            selection_changed: parking_lot::Condvar::new(), garbage: Arc::new(Mutex::new(vec![])),
            row_count: None, texts: vec![], time_indexes: RwLock::new(None), _leases: vec![],
        });
        let a = catalog("Case A", "first"); let b = catalog("Case B", "second");
        let empty = CodesConfig::default();
        // Deterministic old race: A obtained its Session, B refreshed it before
        // A acquired the statement lock. The new guard must restore A itself.
        session.refresh_names(&a, &empty).unwrap();
        session.refresh_names(&b, &empty).unwrap();
        let guard = session.names_guard(&a, &empty).unwrap();
        let name = || session.conn().unwrap().query_row("SELECT name FROM enr", [], |row| row.get::<_, String>(0)).unwrap();
        assert_eq!(name(), "Case A");
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let other = Arc::clone(&session);
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let _guard = other.names_guard(&b, &CodesConfig::default()).unwrap();
            let name: String = other.conn().unwrap().query_row("SELECT name FROM enr", [], |row| row.get(0)).unwrap();
            done_tx.send(name).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(done_rx.recv_timeout(std::time::Duration::from_millis(20)).is_err(), "B cannot refresh during A's pinned statement");
        assert_eq!(name(), "Case A");
        drop(guard);
        assert_eq!(done_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), "Case B");
        worker.join().unwrap();
        let _guard = session.names_guard(&a, &empty).unwrap();
        assert_eq!(name(), "Case A");
        let token = crate::operations::token(Some("case-catalog-cancelled-waiter".into())).unwrap();
        let other = Arc::clone(&session);
        let (started, waiting) = std::sync::mpsc::channel();
        let (finished, result) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || crate::operations::run_with_token(token, || {
            started.send(()).unwrap();
            finished.send(other.names_guard(&catalog("Case B", "second"), &CodesConfig::default()).map(|_| ())).unwrap();
        }));
        waiting.recv().unwrap();
        crate::operations::cancel_id("case-catalog-cancelled-waiter");
        assert!(result.recv_timeout(std::time::Duration::from_secs(5)).unwrap().is_err());
        waiter.join().unwrap();
        assert_eq!(name(), "Case A", "cancelled waiter cannot publish B's enrichment");
    }

    #[test]
    fn derived_store_signature_uses_compiled_security_even_after_context_ends() {
        let derive = |pattern: &str| {
            let mut settings = crate::case_security::Settings::default();
            settings.threat_catalog_json = Some(serde_json::json!({"version":1,"name":"Case","rules":[{"id":"case.signal","name":"Signal","category":"test","severity":"medium","kind":"indicator","pattern":pattern,"description":"Fixture","enabled":true}]}).to_string());
            crate::case_security::compiling(&settings, || vec![CompiledDerived {
                name: "tag".into(), source: "message".into(), steps: vec![], lookup: None,
                rules: vec![crate::sources::CompiledRule::new(regex::Regex::new("(.*)").unwrap(), None,
                    Some(crate::query::Filter { column: "_all".into(), op: "threat_rule".into(), value: "case.signal".into(), value2: None })).unwrap()],
            }])
        };
        let a = derive("ALPHA"); let b = derive("BRAVO");
        let first = derived_signature(&a).unwrap();
        assert_ne!(first, derived_signature(&b).unwrap());
        assert_eq!(first, derived_signature(&derive("ALPHA")).unwrap());
        assert_eq!(derived_signature(&[]).unwrap(), "", "immutable base identity remains unchanged");
        let root = tempfile::tempdir().unwrap(); let file = root.path().join("source.jsonl");
        std::fs::write(&file, "{\"message\":\"ALPHA\"}\n{\"message\":\"BRAVO\"}\n").unwrap();
        let idx = crate::sources::index_file(file.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let codes = CodesConfig::default();
        assert_ne!(spec(&idx, &codes, &codes, &a).unwrap().key, spec(&idx, &codes, &codes, &b).unwrap().key);
        assert_eq!(first, derived_signature(&a).unwrap(), "later/default security cannot re-key captured A");
    }

    #[test]
    fn catalog_token_detects_content_aba_during_admission() {
        let first = catalog("A", "original");
        let second = catalog("B", "changed");
        let empty = CodesConfig::default();
        let before = catalog_token(&first, &empty);
        catalogs_changed();
        let middle = catalog_token(&second, &empty);
        catalogs_changed();
        let after = catalog_token(&first, &empty);
        assert_ne!(before.signature, middle.signature);
        assert_eq!(before.signature, after.signature);
        assert_ne!(before.epoch, after.epoch);
        assert_ne!(before, after);
    }

    #[test]
    fn persisted_catalog_signature_is_stable_across_insertion_order() {
        let mut forward = CodesConfig::default();
        let mut reverse = CodesConfig::default();
        for (target, order) in [(&mut forward, ["a", "b"]), (&mut reverse, ["b", "a"])] {
            for source in order {
                for code in order {
                    target.sources.entry(source.into()).or_default().insert(code.into(), CodeInfo { name: code.into(), description: source.into() });
                }
            }
        }
        let empty = CodesConfig::default();
        assert_eq!(catalogs_signature(&forward, &empty), catalogs_signature(&reverse, &empty));
        assert_eq!(catalog_content_signature(&forward, &empty), catalog_content_signature(&reverse, &empty));
    }
}

#[derive(Clone)]
struct PartSpec {
    key: String,
    path: PathBuf,
    start: usize,
    rows: usize,
    part: usize,
    identity: String,
    first_offset: u64,
    last_end: u64,
}

#[derive(Clone)]
struct SourceSpec {
    key: String,
    parts: Vec<PartSpec>,
    baked: bool,
}

/// Line range of each part (parts are contiguous in `idx.lines`).
fn part_ranges(idx: &FileIndex) -> Option<Vec<(usize, usize)>> {
    let mut ranges = Vec::with_capacity(idx.parts.len());
    let mut next = 0;
    for part in &idx.parts {
        let start = idx.lines.partition_point(|m| m.offset < part.base);
        let end = idx
            .lines
            .partition_point(|m| m.offset < part.base + part.mmap.len() as u64);
        if start != next || end < start {
            return None;
        }
        ranges.push((start, end));
        next = end;
    }
    (next == idx.lines.len()).then_some(ranges)
}

const SEGMENT_ROWS: usize = 1_000_000;
const SEGMENT_BYTES: u64 = 256 << 20;

fn segment_ranges(
    lines: &crate::metadata_store::LineStore,
    start: usize,
    end: usize,
) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = start;
    while from < end {
        let limit = (from + SEGMENT_ROWS).min(end);
        let byte_end = lines.at(from).offset.saturating_add(SEGMENT_BYTES);
        let count = lines.range(from..limit).partition_point(|line| line.offset < byte_end);
        let to = (from + count.max(1)).min(limit);
        out.push((from, to));
        from = to;
    }
    out
}

fn store_ready(part: &PartSpec) -> bool {
    build::published_matches(
        &part.path,
        &part.identity,
        part.rows,
        part.first_offset,
        part.last_end,
    )
}

fn spec(
    idx: &FileIndex,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> Option<SourceSpec> {
    let derived_sig = derived_signature(derived)?;
    let baked = !derived.is_empty();
    let catalogs = if baked {
        catalogs_signature(codes, system)
    } else {
        String::new()
    };
    let tz = chrono::Local::now().offset().to_string();
    let dir = engine_dir();
    let mut parts = Vec::with_capacity(idx.parts.len());
    for (k, (part, (start, end))) in idx.parts.iter().zip(part_ranges(idx)?).enumerate() {
        let custom = match &part.custom {
            Some(crate::sources::CustomParse::Regex(r)) => format!("re:{}", r.as_str()),
            Some(crate::sources::CustomParse::Delimited { sep, fields }) => {
                format!("dl:{sep}{fields:?}")
            }
            None => String::new(),
        };
        let ts = part
            .ts_config
            .as_ref()
            .map(|c| c.signature())
            .unwrap_or_default();
        // Immutable record-aligned segments are independent recovery checkpoints.
        // Keep both row count and input bytes bounded, including very wide logs.
        for (from, to) in segment_ranges(&idx.lines, start, end) {
            let first_offset = idx.lines.at(from).offset - part.base;
            let last = &idx.lines.at(to - 1);
            let last_end = last.offset - part.base + u64::from(last.len);
            let mut hash = Sha256::new();
            hash.update(format!(
                "{}|{}|{}|{}|{:?}|{custom}|{ts}|{tz}|{}|{first_offset}|{last_end}|{derived_sig}|{catalogs}|{:?}",
                build::STORE_VERSION, crate::index_cache::INDEX_DIR, part.identity,
                part.format, part.header, to - from, part.physical_file_id
            ));
            // A current UTC offset alone cannot identify historical local-time
            // rules. Old stores have no proof of this context and are rebuilt.
            hash.update(b"|timezone-configuration:");
            hash.update(part.calendar.timezone.as_bytes());
            if matches!(part.format.as_str(), "syslog3164" | "firewall") {
                hash.update(format!("|inferred-year:{}", part.calendar.year));
            }
            if let Some(identity) = &part.event_identity {
                hash.update(b"|logical-event-identity:");
                hash.update(serde_json::to_vec(identity).ok()?);
            }
            if let Some(revision) = crate::java_stacktrace::enrichment_signature(&part.format) {
                hash.update(b"|parser-enrichment:");
                hash.update(revision.as_bytes());
            }
            // Parser semantics affect SQL/text contents and metadata, but not
            // logical record IDs. Rebuild only affected parser families.
            if let Some(revision) = crate::sources::parser_semantics_signature(&part.format) {
                hash.update(b"|structured-parser:");
                hash.update(revision.as_bytes());
            }
            let key = format!("{:x}", hash.finalize());
            parts.push(PartSpec {
                path: dir.join(format!("{key}.duckdb")),
                key,
                start: from,
                rows: to - from,
                part: k,
                identity: part.identity.clone(),
                first_offset,
                last_end,
            });
        }
    }
    let key = parts
        .iter()
        .map(|p| format!("{}@{}", p.key, p.start))
        .collect::<Vec<_>>()
        .join(",");
    Some(SourceSpec { key, parts, baked })
}

// ---------------------------------------------------------------- sessions

/// Attached stores of the current source, ready for queries.
pub(crate) struct Session {
    key: String,
    base: Mutex<Connection>,
    pool: Mutex<Vec<Connection>>,
    pub(crate) schema: sql::Schema,
    /// Immutable-store proof, established once after every attached store is
    /// validated. False also covers proof failure and keeps null-safe ordering.
    pub(crate) timestamps_non_null: bool,
    baked: bool,
    /// Catalog fingerprint loaded into `enr`; readers hold it while querying names.
    names: RwLock<Option<CatalogKey>>,
    /// Bumped whenever `enr` is reloaded (part of selection keys).
    names_version: AtomicU64,
    /// Recent selections of costly filters, most recent first.
    selections: Mutex<Vec<(String, Arc<ops::Selection>)>>,
    selection_builds: Mutex<HashSet<String>>,
    selection_changed: parking_lot::Condvar,
    /// Selection tables no longer referenced, dropped before new ones are made.
    garbage: Arc<Mutex<Vec<ops::SelectionGarbage>>>,
    /// Verified upper bound for IDs selected from this immutable source.
    /// None is reserved for synthetic test sessions without a SourceSpec.
    row_count: Option<usize>,
    /// Inverted text index of each part with its first line; empty when a
    /// part has none (free text is then scanned).
    texts: Vec<(usize, text::Text)>,
    /// Complete verified capabilities only; acquiring them never reads files.
    time_indexes: RwLock<Option<time_index::ReadSet>>,
    /// Shared OS leases outlive database connections and mapped text readers.
    _leases: Vec<std::fs::File>,
}

/// Best-effort pressure recovery across the current and retained sessions.
/// Acquire Arcs under Registry, then release that lock before any SQL, cache
/// mutation or credit release. A retired active session is supplied explicitly.
pub(crate) fn trim_inactive_selection_caches(current: Option<&Session>, pressure: Option<(&crate::case_work_budget::Pool, crate::case_work_budget::Pressure)>) -> Result<(), String> {
    let sessions = with_registry(|registry| {
        let mut sessions = Vec::<Arc<Session>>::new();
        for session in registry.base_session.iter().chain(registry.session.iter()) {
            if !current.is_some_and(|current| std::ptr::eq(session.as_ref(), current))
                && !sessions.iter().any(|held| Arc::ptr_eq(held, session)) {
                sessions.push(Arc::clone(session));
            }
        }
        sessions
    });
    let mut failure = current.and_then(|current| current.trim_inactive_selections_for(pressure).err());
    for session in sessions {
        if let Err(error) = session.trim_inactive_selections_for(pressure) {
            if failure.is_none() { failure = Some(error); }
        }
    }
    failure.map_or(Ok(()), Err)
}

pub(crate) struct Pooled<'a> {
    session: &'a Session,
    conn: Option<Connection>,
}

impl std::ops::Deref for Pooled<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn.as_ref().expect("pooled connection")
    }
}

impl Pooled<'_> {
    /// Never recycle a connection whose transaction cleanup failed.
    pub(crate) fn discard(&mut self) { self.conn.take(); }
}

pub(crate) struct SelectionBuild<'a> { session: &'a Session, key: String }
impl Drop for SelectionBuild<'_> {
    fn drop(&mut self) {
        self.session.selection_builds.lock().remove(&self.key);
        self.session.selection_changed.notify_all();
    }
}

impl Drop for Pooled<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            let mut pool = self.session.pool.lock();
            if pool.len() < 8 {
                pool.push(conn);
            }
        }
    }
}

/// An existence probe can reject a nullable source as soon as one row matches;
/// DuckDB can prove the null-free case from each segment's column statistics.
/// This is an optional optimization: query/proof failures retain the old path.
fn timestamp_non_null_proof(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT NOT EXISTS (SELECT 1 FROM ev WHERE ts IS NULL LIMIT 1)",
        [],
        |row| row.get::<_, bool>(0),
    ).unwrap_or(false)
}

impl Session {
    fn open(spec: &SourceSpec, time_indexes: Option<time_index::ReadSet>) -> Result<Session, String> {
        let mut leases = Vec::with_capacity(spec.parts.len());
        for part in &spec.parts {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(part.path.with_extension("build.lock"))
                .map_err(|e| e.to_string())?;
            fs2::FileExt::try_lock_shared(&file).map_err(|_| {
                "Checkpoint ocupado; preparação ou limpeza em andamento.".to_string()
            })?;
            if !store_ready(part) {
                return Err("Checkpoint incompleto ou inválido; retome a preparação.".into());
            }
            leases.push(file);
        }
        let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
        let temp = engine_dir().join("tmp");
        let _ = std::fs::create_dir_all(&temp);
        conn.execute_batch(&format!(
            "SET temp_directory = {}; SET preserve_insertion_order = false;",
            sql::lit(&temp.to_string_lossy())
        ))
        .map_err(|e| e.to_string())?;
        limit_resources(&conn, false)?;
        udf::register(&conn).map_err(|e| e.to_string())?;
        let mut schema = sql::Schema::default();
        let mut columns: HashMap<String, String> = HashMap::new();
        let mut overflow: HashSet<String> = HashSet::new();
        let mut selects = Vec::new();
        let mut baked = spec.baked;
        let mut texts: Vec<(usize, text::Text)> = spec
            .parts
            .iter()
            .filter_map(|part| text::Text::open(&text::dir_of(&part.path)).map(|t| (part.start, t)))
            .collect();
        if texts.len() != spec.parts.len() {
            texts.clear();
        }
        for (k, part) in spec.parts.iter().enumerate() {
            // Touch the store (before it is opened) so the cache keeps recently used ones.
            let _ = std::fs::File::options()
                .append(true)
                .create(true)
                .open(part.path.with_extension("used"))
                .and_then(|f| f.set_modified(std::time::SystemTime::now()));
            conn.execute_batch(&format!(
                "ATTACH {} AS p{k} (READ_ONLY)",
                sql::lit(&part.path.to_string_lossy())
            ))
            .map_err(|e| e.to_string())?;
            let info = build::read_info(&conn, &format!("p{k}"))?;
            if info.rows != part.rows {
                return Err("Índice de consultas não corresponde ao arquivo.".into());
            }
            baked &= info.baked;
            let mut select = format!(
                "SELECT lid::BIGINT + {} AS id, ts, lvl, level, source, code, message, pname, pdesc, \
                 COALESCE(eref, {} || off::VARCHAR) AS event_ref, vals, pat",
                part.start,
                sql::lit(&format!("{}:", part.identity)),
            );
            for info_role in crate::entities::ROLES {
                select.push_str(&format!(", {}", build::role_column(info_role.column)));
            }
            select.push_str(&format!(", {}", build::QUERY_TOOL));
            for (field, column) in &info.wide {
                let next = columns.len();
                let global = columns
                    .entry(field.clone())
                    .or_insert_with(|| format!("w{next}"));
                select.push_str(&format!(", {column} AS {global}"));
            }
            select.push_str(&format!(" , xk, xv FROM p{k}.ev"));
            selects.push(select);
            overflow.extend(info.overflow);
            schema.structured.extend(info.structured);
            schema.ci_multi.extend(info.ci_multi);
        }
        let mut names: HashSet<&String> = columns.keys().collect();
        names.extend(overflow.iter());
        for name in names {
            let wide = columns.get(name);
            let spilled = overflow
                .contains(name)
                .then(|| format!("xv[list_position(xk, {})]", sql::lit(name)));
            let expr = match (wide, spilled) {
                (Some(w), Some(o)) => format!("COALESCE({w}, {o})"),
                (Some(w), None) => w.clone(),
                (None, Some(o)) => o,
                (None, None) => continue,
            };
            schema.fields.insert(name.clone(), expr);
            schema
                .lower
                .entry(name.to_ascii_lowercase())
                .or_default()
                .push(name.clone());
        }
        let union = selects.join(" UNION ALL BY NAME ");
        let names_view = if baked {
            "CREATE VIEW evn AS SELECT *, pname AS name, pdesc AS description FROM ev;".to_string()
        } else {
            "CREATE TABLE enr (source VARCHAR, code VARCHAR, name VARCHAR, description VARCHAR, catalog INTEGER); \
             CREATE VIEW evn AS SELECT ev.*, \
             COALESCE(ue.name, uw.name, se.name, sw.name, ev.pname) AS name, \
             COALESCE(ue.description, uw.description, se.description, sw.description, ev.pdesc) AS description FROM ev \
             LEFT JOIN enr ue ON ue.catalog=0 AND ue.source=ev.source AND ue.code=ev.code \
             LEFT JOIN enr uw ON uw.catalog=0 AND uw.source='*' AND uw.code=ev.code \
             LEFT JOIN enr se ON se.catalog=1 AND se.source=ev.source AND se.code=ev.code \
             LEFT JOIN enr sw ON sw.catalog=1 AND sw.source='*' AND sw.code=ev.code;".to_string()
        };
        conn.execute_batch(&format!("CREATE VIEW ev AS {union}; {names_view}"))
            .map_err(|e| e.to_string())?;
        // All parts have already passed store_ready/read_info and are held
        // under shared publication leases. The attached tables are read-only,
        // so this proof remains valid for the whole Session (including evn).
        let timestamps_non_null = timestamp_non_null_proof(&conn);
        Ok(Session {
            key: spec.key.clone(),
            base: Mutex::new(conn),
            pool: Mutex::new(Vec::new()),
            schema,
            timestamps_non_null,
            baked,
            names: RwLock::new(None),
            names_version: AtomicU64::new(0),
            selections: Mutex::new(Vec::new()),
            selection_builds: Mutex::new(HashSet::new()),
            selection_changed: parking_lot::Condvar::new(),
            garbage: Arc::new(Mutex::new(Vec::new())),
            row_count: Some(spec.parts.iter().map(|part| part.rows).sum()),
            texts,
            time_indexes: RwLock::new(time_indexes),
            _leases: leases,
        })
    }

    pub(crate) fn exact_time_indexes(&self) -> Option<time_index::ReadSet> {
        self.time_indexes.read().clone()
    }

    pub(crate) fn invalidate_exact_times(&self, readers: &time_index::ReadSet) -> Result<(), String> {
        // Probe outside Registry. Only the precise stale Arc generations are
        // evicted; a concurrent, freshly verified replacement remains usable.
        let stale = time_index::changed_readers(readers)?;
        if stale.is_empty() { return Ok(()); }
        let clear = |session: &Session| {
            let mut current = session.time_indexes.write();
            let changed = current.as_ref().is_some_and(|set| time_index::shares_generation(set, &stale));
            if changed { *current = None; }
            changed
        };
        with_registry(|reg| {
            reg.time_cache.remove_matching(&stale);
            let mut retry = Vec::new();
            for session in reg.base_session.iter().chain(reg.session.iter()) {
                if clear(session) { retry.push(session.key.clone()); }
            }
            if clear(self) { retry.push(self.key.clone()); }
            for key in retry { reg.time_attempts.remove(&key); }
        });
        // The caller drops its old ReadSet before exact SQL fallback. The next
        // current-source request can enqueue repair without an own-reader lock.
        Ok(())
    }

    /// Lines (sorted) whose free text may contain `needle`, through the
    /// inverted indexes; `None` when they cannot narrow the search.
    pub(crate) fn free_candidates(&self, needle: &str, limit: usize) -> Result<Option<Vec<usize>>, String> {
        self.text_candidates(limit, |text, remaining| text.candidates(needle, remaining))
    }

    pub(crate) fn exact_hex_candidates(&self, value: &str, limit: usize) -> Result<Option<Vec<usize>>, String> {
        self.text_candidates(limit, |text, remaining| text.exact_hex_candidates(value, remaining))
    }

    fn text_candidates(&self, limit: usize, mut probe: impl FnMut(&text::Text, usize) -> Option<Vec<u32>>) -> Result<Option<Vec<usize>>, String> {
        crate::operations::check()?;
        if self.texts.is_empty() { return Ok(None); }
        let mut out = Vec::new();
        for (start, text) in &self.texts {
            crate::operations::check()?;
            let candidates = probe(text, limit - out.len());
            // A cancelled probe must not fall through to a full SQL scan.
            crate::operations::check()?;
            let Some(lids) = candidates else { return Ok(None) };
            out.extend(lids.into_iter().map(|lid| start + lid as usize));
        }
        Ok(Some(out))
    }

    pub(crate) fn names_version(&self) -> u64 {
        self.names_version.load(Ordering::SeqCst)
    }

    pub(crate) fn cached_selection(&self, key: &str) -> Option<Arc<ops::Selection>> {
        let key = format!("{}|{key}", crate::analysis_runtime::cache_namespace());
        let mut selections = self.selections.lock();
        let at = selections.iter().position(|(k, _)| k == &key)?;
        let entry = selections.remove(at);
        let found = Arc::clone(&entry.1);
        selections.insert(0, entry);
        Some(found)
    }

    pub(crate) fn selection_snapshot(&self) -> serde_json::Value {
        let selections = self.selections.lock();
        serde_json::json!({
            "entries": selections.len(),
            "accountedBytes": selections.iter().map(|(_, s)| s.accounted_bytes()).sum::<u64>(),
            "tables": selections.iter().map(|(_, s)| s.name()).collect::<Vec<_>>(),
        })
    }

    pub(crate) fn cache_selection(&self, key: String, selection: Arc<ops::Selection>) {
        let key = format!("{}|{key}", crate::analysis_runtime::cache_namespace());
        let owner_budget = crate::resources::selection_cache_bytes();
        let aggregate_budget = crate::resources::application_selection_cache_bytes();
        let mut retired = Vec::new();
        let mut selections = self.selections.lock();
        for index in (0..selections.len()).rev() {
            if selections[index].0 == key { retired.push(selections.remove(index)); }
        }
        if selection.accounted_bytes() <= owner_budget && selection.accounted_bytes() <= aggregate_budget {
            selections.insert(0, (key, Arc::clone(&selection)));
            loop {
                let owner_bytes = selections.iter().filter(|(_, entry)| selection.same_owner(entry))
                    .fold(0u64, |bytes, (_, entry)| bytes.saturating_add(entry.accounted_bytes()));
                let aggregate_bytes = selections.iter().fold(0u64, |bytes, (_, entry)| bytes.saturating_add(entry.accounted_bytes()));
                let owner_pressure = owner_bytes > owner_budget;
                if !owner_pressure && selections.len() <= 8 && aggregate_bytes <= aggregate_budget { break; }
                let index = if owner_pressure {
                    selections.iter().rposition(|(_, entry)| selection.same_owner(entry)).expect("current owner entry")
                } else { selections.len() - 1 };
                retired.push(selections.remove(index));
            }
        }
        // Cache eviction transfers credits to garbage outside the cache lock;
        // the table's actual DROP remains responsible for releasing them.
        drop(selections);
        drop(retired);
    }

    pub(crate) fn begin_selection(&self, key: &str) -> Result<SelectionBuild<'_>, String> {
        let key = format!("{}|{key}", crate::analysis_runtime::cache_namespace());
        let mut building = self.selection_builds.lock();
        while building.contains(&key) {
            crate::operations::check()?;
            self.selection_changed.wait_for(&mut building, std::time::Duration::from_millis(25));
        }
        crate::operations::check()?;
        building.insert(key.to_owned());
        Ok(SelectionBuild { session: self, key: key.to_owned() })
    }

    pub(crate) fn row_upper_bound(&self) -> Option<usize> { self.row_count }

    /// Pressure recovery must not discard a table still held by a query.
    /// Drop cache-owned Arcs outside the cache lock, then retire the actual
    /// SQL tables before their logical-selection credits can be reused.
    pub(crate) fn trim_inactive_selections(&self) -> Result<(), String> {
        self.trim_inactive_selections_for(None)
    }
    fn trim_inactive_selections_for(&self, pressure: Option<(&crate::case_work_budget::Pool, crate::case_work_budget::Pressure)>) -> Result<(), String> {
        let removed = {
            let mut selections = self.selections.lock();
            let mut kept = Vec::with_capacity(selections.len());
            let mut removed = Vec::new();
            for entry in selections.drain(..) {
                if Arc::strong_count(&entry.1) == 1
                    && pressure.is_none_or(|(pool, scope)| entry.1.releases_pressure(pool, scope)) { removed.push(entry); }
                else { kept.push(entry); }
            }
            *selections = kept;
            removed
        };
        drop(removed);
        self.collect_garbage()
    }

    pub(crate) fn garbage(&self) -> Arc<Mutex<Vec<ops::SelectionGarbage>>> {
        Arc::clone(&self.garbage)
    }

    pub(crate) fn take_garbage(&self) -> Vec<ops::SelectionGarbage> {
        std::mem::take(&mut *self.garbage.lock())
    }

    pub(crate) fn collect_garbage(&self) -> Result<(), String> {
        if self.garbage.lock().is_empty() { return Ok(()); }
        // A failed connection acquisition must leave the cleanup queue intact.
        let conn = self.conn()?;
        self.collect_garbage_on(&conn)
    }

    pub(crate) fn collect_garbage_on(&self, conn: &Connection) -> Result<(), String> {
        let mut unused = self.take_garbage().into_iter();
        while let Some(item) = unused.next() {
            let dropped = crate::operations::check().and_then(|_| {
                conn.execute_batch(&format!("DROP TABLE IF EXISTS {}", item.name)).map_err(|error| error.to_string())
            });
            if let Err(error) = dropped {
                let mut pending = self.garbage.lock();
                pending.push(item);
                pending.extend(unused);
                return Err(error);
            }
            // A dropped cache Arc is not proof the SQL table has disappeared.
            // Release the carried credit only after successful DROP.
            drop(item);
        }
        Ok(())
    }

    pub(crate) fn conn(&self) -> Result<Pooled<'_>, String> {
        let conn = match self.pool.lock().pop() {
            Some(conn) => conn,
            None => self.base.lock().try_clone().map_err(|e| e.to_string())?,
        };
        Ok(Pooled {
            session: self,
            conn: Some(conn),
        })
    }

    /// Refresh plus statement pin are one atomic operation. A shared immutable
    /// base can serve two Cases, but its enrichment table is mutable; a bare
    /// read lock taken after session acquisition could pin the other Case.
    fn refresh_names(&self, codes: &CodesConfig, system: &CodesConfig) -> Result<(), String> {
        drop(self.names_guard(codes, system)?);
        Ok(())
    }
    pub(crate) fn names_guard(&self, codes: &CodesConfig, system: &CodesConfig)
        -> Result<parking_lot::RwLockReadGuard<'_, Option<CatalogKey>>, String>
    {
        crate::operations::check()?;
        let current = loop {
            crate::operations::check()?;
            if let Some(guard) = self.names.try_read_for(std::time::Duration::from_millis(25)) {
                crate::operations::check()?;
                break guard;
            }
        };
        if self.baked { return Ok(current); }
        let key = catalog_key(codes, system);
        if current.as_ref() == Some(&key) { return Ok(current); }
        drop(current);
        let mut current = loop {
            crate::operations::check()?;
            if let Some(guard) = self.names.try_write_for(std::time::Duration::from_millis(25)) {
                crate::operations::check()?;
                break guard;
            }
        };
        if current.as_ref() == Some(&key) {
            return Ok(parking_lot::RwLockWriteGuard::downgrade(current));
        }
        let mut conn = self.conn()?;
        conn.execute_batch("BEGIN TRANSACTION").map_err(|e| e.to_string())?;
        let result: Result<(), String> = (|| {
            conn.execute_batch("DELETE FROM enr").map_err(|e| e.to_string())?;
            let mut appender = conn.appender("enr").map_err(|e| e.to_string())?;
            let mut budget = crate::query::AnalyticsBudget::new();
            for (catalog, config) in [codes, system].iter().enumerate() {
                for (source, entries) in &config.sources {
                    for (code, info) in entries {
                        if code.is_empty() { continue; }
                        crate::operations::check()?;
                        budget.charge(source.len().saturating_add(code.len()).saturating_add(info.name.len()).saturating_add(info.description.len()).saturating_add(64))?;
                        appender.append_row(duckdb::params![source, code, info.name, info.description, catalog as i32]).map_err(|e| e.to_string())?;
                    }
                }
            }
            appender.flush().map_err(|e| e.to_string())?;
            drop(appender);
            crate::operations::check()?;
            conn.execute_batch("COMMIT").map_err(|e| e.to_string())
        })();
        if result.is_err() && conn.execute_batch("ROLLBACK").is_err() { conn.discard(); }
        result?;
        *current = Some(key);
        self.names_version.fetch_add(1, Ordering::SeqCst);
        self.selections.lock().clear();
        Ok(parking_lot::RwLockWriteGuard::downgrade(current))
    }
}

/// Apply coordinated worker/memory budgets and a separate spill ceiling.
/// Source metadata, persistent stores and allocator overhead are additional.
pub(crate) fn limit_resources(conn: &Connection, build: bool) -> Result<(), String> {
    let megabytes = crate::resources::duckdb_memory_mb();
    let directory = engine_dir().join("tmp");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let spill = crate::resources::spill_bytes(build, &directory)?;
    conn.execute_batch(&format!(
        "SET threads = {}; SET memory_limit = '{megabytes}MB'; SET max_temp_directory_size = '{spill}B';",
        crate::resources::query_threads(),
    )).map_err(|e| e.to_string())
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BuildProgress {
    pub phase: String,
    pub completed: usize,
    pub total: usize,
    pub checkpoint_rows: usize,
    pub completed_segments: usize,
    pub total_segments: usize,
    pub resumed_rows: usize,
    pub state: String,
    pub error: Option<String>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EngineStatus {
    pub state: String,
    pub base_ready: bool,
    pub derived_ready: bool,
    pub phase: String,
    pub completed_rows: usize,
    pub total_rows: usize,
    pub completed_segments: usize,
    pub total_segments: usize,
    pub resumed_rows: usize,
    pub can_resume: bool,
    pub error: Option<String>,
}

/// Does not schedule work. Readiness never represents a partially built dataset
/// as complete; counts refer to verified durable segments of the current config.
pub(crate) fn status(
    idx: &FileIndex,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> EngineStatus {
    let base = spec(idx, codes, system, &[]);
    let desired = spec(idx, codes, system, derived);
    let base_ready = base
        .as_ref()
        .is_some_and(|s| !s.parts.is_empty() && s.parts.iter().all(store_ready));
    let mut out = EngineStatus {
        state: "indexing".into(),
        base_ready,
        derived_ready: false,
        phase: "Aguardando preparação".into(),
        completed_rows: 0,
        total_rows: idx.lines.len(),
        completed_segments: 0,
        total_segments: 0,
        resumed_rows: 0,
        can_resume: true,
        error: None,
    };
    if let Some(error) = idx
        .parts
        .iter()
        .find_map(|part| crate::sources::validate_source(part).err())
    {
        out.state = "stale".into();
        out.base_ready = false;
        out.can_resume = false;
        out.phase = "Fonte alterada; reabra o arquivo".into();
        out.error = Some(error);
        return out;
    }
    if !enabled() {
        out.state = "disabled".into();
        out.can_resume = false;
        return out;
    }
    let Some(spec) = desired else {
        out.state = "degraded".into();
        out.can_resume = false;
        out.error = Some("A configuração de campos derivados exige o motor de linhas.".into());
        return out;
    };
    out.total_segments = spec.parts.len();
    for part in &spec.parts {
        if store_ready(part) {
            out.completed_rows += part.rows;
            out.completed_segments += 1;
        }
    }
    out.derived_ready = !spec.parts.is_empty() && out.completed_segments == out.total_segments;
    let (progress, error) = with_registry(|reg| {
        (
            reg.progress.get(&spec.key).cloned().or_else(|| {
                spec.parts
                    .iter()
                    .chain(base.iter().flat_map(|s| &s.parts))
                    .find_map(|part| {
                        reg.progress
                            .get(&part.key)
                            .filter(|p| p.state != "ready")
                            .cloned()
                    })
            }),
            spec.parts
                .iter()
                .find_map(|p| reg.failed.get(&p.key).cloned())
                .or_else(|| reg.failed.get(&spec.key).cloned()),
        )
    });
    if let Some(progress) = progress {
        out.phase = progress.phase;
        out.resumed_rows = progress.resumed_rows;
        out.state = progress.state;
        out.error = progress.error;
    }
    if let Some(error) = error {
        out.state = "degraded".into();
        out.error = Some(error);
    }
    finish_status(out)
}

fn finish_status(mut out: EngineStatus) -> EngineStatus {
    if out.error.is_some() {
        out.state = "degraded".into();
        out.derived_ready = false;
    } else if out.derived_ready {
        out.state = "ready".into();
        out.phase = "Consultas prontas".into();
        out.can_resume = false;
    }
    out
}

/// Clears only failures for this source/configuration. Completed checkpoints
/// remain intact and are validated/reused on the next prepare call.
pub(crate) fn retry(
    idx: &FileIndex,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) {
    for fields in [&[][..], derived] {
        if let Some(spec) = spec(idx, codes, system, fields) {
            with_registry(|reg| {
                reg.failed.remove(&spec.key);
                for part in spec.parts {
                    reg.failed.remove(&part.key);
                }
            });
        }
    }
}

pub(crate) fn base_session(
    idx: &FileIndex,
    codes: &CodesConfig,
    system: &CodesConfig,
) -> Option<Arc<Session>> {
    session(idx, codes, system, &[])
}

// ---------------------------------------------------------------- registry

#[derive(Default)]
struct Registry {
    session: Option<Arc<Session>>,
    base_session: Option<Arc<Session>>,
    building: HashSet<String>,
    failed: HashMap<String, String>,
    /// Stores of the source in use; background builds of others stop.
    wanted: HashSet<String>,
    source_identity: String,
    /// Preparation can run before source publication, so readiness/error keys
    /// need ownership independent of the currently opened session's identity.
    source_keys: HashMap<String, HashSet<String>>,
    progress: HashMap<String, BuildProgress>,
    time_cache: time_index::VerifiedCache,
    time_attempts: HashMap<String, std::time::Instant>,
}

impl Registry {
    fn install_times(&mut self, spec: &SourceSpec) {
        let Some(readers) = self.time_cache.complete(spec.parts.iter().map(|part|part.key.as_str())) else { return; };
        for session in self.base_session.iter().chain(self.session.iter()) {
            if session.key == spec.key { *session.time_indexes.write() = Some(Arc::clone(&readers)); }
        }
    }
    fn remember_spec(&mut self, idx: &FileIndex, spec: &SourceSpec) {
        self.source_keys
            .entry(source_identity(idx))
            .or_default()
            .insert(spec.key.clone());
        // Segment keys survive append/merge: their ownership is the physical
        // constituent source, independent of its global row offset.
        for part in &spec.parts {
            self.source_keys
                .entry(part_source_identity(&idx.parts[part.part]))
                .or_default()
                .insert(part.key.clone());
        }
    }

    fn source_published(&mut self, idx: Option<&FileIndex>, queue: Option<&BackgroundQueue>) {
        let idx = idx.filter(|idx| !idx.lines.is_empty());
        let identity = idx.map(source_identity);
        let owners = idx
            .map(|idx| {
                idx.parts
                    .iter()
                    .map(part_source_identity)
                    .chain(std::iter::once(source_identity(idx)))
                    .collect()
            })
            .unwrap_or_default();
        self.retain_source(identity.as_deref(), &owners);
        if let Some(queue) = queue {
            // Same lock order as schedule: registry, then queue. The worker
            // never holds the queue lock while acquiring the registry.
            queue.retain_source(identity.as_deref());
        }
    }

    fn retain_source(&mut self, identity: Option<&str>, owners: &HashSet<String>) {
        if identity.is_none_or(|identity| self.source_identity != identity) {
            self.session = None;
            self.base_session = None;
        }
        // A failed incoming preparation still publishes a valid line source.
        // Keep its degraded cause/progress and wanted keys even though no
        // successful Session::open has set source_identity to it yet. Prune
        // ownership even when a successful session already changed identity.
        self.source_keys.retain(|owner, _| owners.contains(owner));
        let keep: HashSet<&String> = self
            .source_keys
            .values()
            .flat_map(|keys| keys.iter())
            .collect();
        self.wanted.retain(|key| keep.contains(key));
        self.failed.retain(|key, _| keep.contains(key));
        self.progress.retain(|key, _| keep.contains(key));
        self.time_cache.retain(|key| keep.contains(key));
        self.time_attempts.retain(|key,_| keep.contains(key));
        self.source_identity = identity.unwrap_or_default().to_string();
        // Active builders keep their claims until they unwind. Dropping the
        // claim here could let a foreground retry race the same segment.
    }
}

/// Publish the UI source lifecycle while its write guard is held. Preparation
/// may already have opened this source's sessions: keep those and its queued
/// work, but release all state belonging to a cleared or replaced source.
/// Existing query Arcs and an active native call live until they return.
pub(crate) fn source_published(idx: Option<&FileIndex>) {
    with_registry(|reg| {
        reg.source_published(idx, BACKGROUND_QUEUE.get().map(Arc::as_ref));
    });
}

fn still_wanted(key: &str) -> bool {
    with_registry(|reg| reg.wanted.contains(key))
}

static REGISTRY: Mutex<Option<Registry>> = parking_lot::const_mutex(None);

fn with_registry<T>(f: impl FnOnce(&mut Registry) -> T) -> T {
    let mut guard = REGISTRY.lock();
    f(guard.get_or_insert_with(Registry::default))
}

/// Ready session for the source, or `None` while its stores are missing
/// (they are then built in the background and the line engine answers).
pub(crate) fn session(idx: &FileIndex, codes: &CodesConfig, system: &CodesConfig, derived: &[CompiledDerived]) -> Option<Arc<Session>> {
    session_checked(idx, codes, system, derived).ok().flatten()
}

pub(crate) fn session_checked(
    idx: &FileIndex,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> Result<Option<Arc<Session>>, String> {
    if !enabled() || idx.lines.is_empty() {
        return Ok(None);
    }
    let base = if derived.is_empty() {
        None
    } else {
        spec(idx, codes, system, &[])
    };
    let Some(spec) = spec(idx, codes, system, derived) else { return Ok(None); };
    if let Some(error) = idx
        .parts
        .iter()
        .find_map(|part| crate::sources::validate_source(part).err())
    {
        with_registry(|reg| {
            reg.failed.insert(spec.key.clone(), error.clone());
        });
        return Err(error);
    }
    let session = with_registry(|reg| -> Result<Option<Arc<Session>>, String> {
        let identity = source_identity(idx);
        reg.remember_spec(idx, &spec);
        if let Some(base) = &base {
            reg.remember_spec(idx, base);
        }
        if reg.source_identity != identity {
            reg.wanted.clear();
            reg.progress.clear();
            reg.session = None;
            reg.base_session = None;
            reg.source_identity = identity;
        }
        if !derived.is_empty() {
            reg.wanted = base
                .iter()
                .flat_map(|s| &s.parts)
                .map(|p| p.key.clone())
                .collect();
        }
        reg.wanted.extend(spec.parts.iter().map(|p| p.key.clone()));
        let cached = if derived.is_empty() {
            &reg.base_session
        } else {
            &reg.session
        };
        if let Some(current) = cached {
            if current.key == spec.key {
                let current = Arc::clone(current);
                maybe_schedule_time(reg, idx, &spec, base.as_ref(), derived, codes, system);
                return Ok(Some(current));
            }
        }
        if let Some(error) = reg.failed.get(&spec.key) {
            return Err(error.clone());
        }
        let missing: Vec<&PartSpec> = spec.parts.iter().filter(|p| !store_ready(p)).collect();
        if !missing.is_empty() {
            if missing
                .iter()
                .any(|part| !reg.failed.contains_key(&part.key))
            {
                schedule(reg, idx, &spec, base.as_ref(), derived, codes, system);
            }
            return Ok(None);
        }
        let time_indexes = reg.time_cache.complete(spec.parts.iter().map(|part|part.key.as_str()));
        match Session::open(&spec, time_indexes) {
            Ok(session) => {
                let session = Arc::new(session);
                if derived.is_empty() {
                    reg.base_session = Some(Arc::clone(&session));
                } else {
                    reg.session = Some(Arc::clone(&session));
                }
                maybe_schedule_time(reg, idx, &spec, base.as_ref(), derived, codes, system);
                Ok(Some(session))
            }
            Err(error) => {
                eprintln!("[motor] sessão indisponível: {error}");
                if error.starts_with("Checkpoint ocupado") { return Ok(None); }
                reg.failed.insert(spec.key.clone(), error.clone());
                Err(error)
            }
        }
    })?;
    let Some(session) = session else { return Ok(None); };
    session.refresh_names(codes, system)?;
    Ok(Some(session))
}

struct Job {
    key: String,
    target: PathBuf,
    source: build::PartSource,
    derived: Vec<CompiledDerived>,
    catalogs: Option<(CodesConfig, CodesConfig)>,
}

/// A base-safe first page can finish without opening a derived Session. Queue
/// its captured desired variant afterward, without clearing failures or changing
/// the registry owner. Source publication remains the ownership authority.
pub(crate) fn ensure_admitted_variant(idx: &FileIndex, codes: &CodesConfig, system: &CodesConfig, derived: &[CompiledDerived]) {
    if !enabled() || idx.lines.is_empty() || derived.is_empty() { return; }
    let Some(desired) = spec(idx, codes, system, derived) else { return; };
    let base = spec(idx, codes, system, &[]);
    let identity = source_identity(idx);
    with_registry(|reg| {
        if reg.source_identity != identity || reg.failed.contains_key(&desired.key)
            || desired.parts.iter().any(|part| reg.failed.contains_key(&part.key))
            || desired.parts.iter().all(store_ready) { return; }
        reg.remember_spec(idx, &desired);
        if let Some(base) = &base { reg.remember_spec(idx, base); }
        reg.wanted = base.iter().flat_map(|spec| &spec.parts).chain(&desired.parts).map(|part| part.key.clone()).collect();
        schedule(reg, idx, &desired, base.as_ref(), derived, codes, system);
    });
}

/// Called when definitions change, independent of which UI query runs next.
/// Preparation remains background work and does not evict the immutable base.
pub(crate) fn request_rebuild(
    idx: &FileIndex,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) {
    if !enabled() || idx.lines.is_empty() {
        return;
    }
    let base = if derived.is_empty() {
        None
    } else {
        spec(idx, codes, system, &[])
    };
    let Some(desired) = spec(idx, codes, system, derived) else {
        return;
    };
    let identity = source_identity(idx);
    with_registry(|reg| {
        reg.remember_spec(idx, &desired);
        if let Some(base) = &base {
            reg.remember_spec(idx, base);
        }
        if reg.source_identity != identity {
            reg.base_session = None;
            reg.session = None;
            reg.progress.clear();
            reg.source_identity = identity;
        }
        reg.wanted = base
            .iter()
            .flat_map(|s| &s.parts)
            .chain(desired.parts.iter())
            .map(|p| p.key.clone())
            .collect();
        reg.failed.remove(&desired.key);
        for part in &desired.parts {
            reg.failed.remove(&part.key);
        }
        if reg.session.as_ref().is_some_and(|s| s.key != desired.key) {
            reg.session = None;
        }
        schedule(reg, idx, &desired, base.as_ref(), derived, codes, system);
    });
}

fn part_source_identity(part: &crate::sources::FilePart) -> String {
    format!(
        "{}|{}|{:?}|{}",
        part.identity,
        part.format,
        part.physical_file_id,
        part.ts_config
            .as_ref()
            .map(|c| c.signature())
            .unwrap_or_default()
    )
}

fn source_identity(idx: &FileIndex) -> String {
    idx.parts
        .iter()
        .map(part_source_identity)
        .collect::<Vec<_>>()
        .join(",")
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RequestKey {
    source: String,
    config: String,
    derived: bool,
}

/// One pending latest request replaces all older queued segment jobs. The
/// active request is cooperatively superseded; each iterates its own segments.
struct BackgroundRequest {
    key: RequestKey,
    revision: u64,
    idx: FileIndex,
    spec: SourceSpec,
    base: Option<SourceSpec>,
    derived: Vec<CompiledDerived>,
    codes: CodesConfig,
    system: CodesConfig,
}
#[derive(Default)]
struct QueueState {
    pending: Option<BackgroundRequest>,
    active: Option<RequestKey>,
}
struct BackgroundQueue {
    state: Mutex<QueueState>,
    wake: parking_lot::Condvar,
    revision: AtomicU64,
    running: AtomicBool,
}

static BACKGROUND_QUEUE: std::sync::OnceLock<Arc<BackgroundQueue>> = std::sync::OnceLock::new();

impl BackgroundQueue {
    fn retain_source(&self, source: Option<&str>) {
        let mut state = self.state.lock();
        let keep_pending = state
            .pending
            .as_ref()
            .is_some_and(|request| source.is_some_and(|source| request.key.source == source));
        let keep_active = state
            .active
            .as_ref()
            .is_some_and(|request| source.is_some_and(|source| request.source == source));
        let removed_pending = state.pending.is_some() && !keep_pending;
        if !keep_pending {
            // Drop shared metadata/mappings immediately for work not started.
            state.pending = None;
        }
        if !keep_active || removed_pending {
            // A pending request had already superseded the active revision;
            // removing it cannot resurrect that active build. Let a future
            // request for the same key enqueue instead of deduplicating it.
            state.active = None;
        }
        if !keep_pending && (!keep_active || removed_pending) {
            self.revision.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// Stop only this app's cooperative work. Never clear the loaded source,
/// delete checkpoints, or terminate another process to install an update.
pub(crate) fn prepare_for_update() -> Result<crate::operations::UpdatePause, String> {
    let pause = crate::operations::pause_for_update()?;
    if let Some(queue) = BACKGROUND_QUEUE.get() {
        let mut state = queue.state.lock();
        state.pending = None;
        queue.revision.fetch_add(1, Ordering::SeqCst);
        queue.wake.notify_all();
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let building = with_registry(|reg| !reg.building.is_empty());
        let background = BACKGROUND_QUEUE.get().is_some_and(|queue| queue.running.load(Ordering::Acquire));
        if !building && !background && !crate::operations::update_work_active() { return Ok(pause); }
        if std::time::Instant::now() >= deadline {
            return Err("O trabalho em andamento ainda não confirmou o encerramento. A aplicação continua aberta; aguarde a tarefa terminar e tente instalar novamente.".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

fn supersedes(new: &RequestKey, current: &RequestKey) -> bool {
    new != current && !(new.source == current.source && !new.derived && current.derived)
}

fn schedule(
    reg: &mut Registry,
    idx: &FileIndex,
    spec: &SourceSpec,
    base: Option<&SourceSpec>,
    derived: &[CompiledDerived],
    codes: &CodesConfig,
    system: &CodesConfig,
) {
    if crate::operations::update_paused() { return; }
    // All callers hold Registry's mutex, so initialization is serialized.
    // A missing optional-worker resource must not panic an otherwise ready query.
    let queue = if let Some(queue) = BACKGROUND_QUEUE.get() { queue } else {
        let queue = Arc::new(BackgroundQueue {
            state: Mutex::new(QueueState::default()),
            wake: parking_lot::Condvar::new(),
            revision: AtomicU64::new(0),
            running: AtomicBool::new(false),
        });
        let worker = Arc::clone(&queue);
        let spawned = std::thread::Builder::new()
            .name("loginsight-engine".into())
            .spawn(move || {
                crate::resources::lower_priority();
                loop {
                    let request = {
                        let mut state = worker.state.lock();
                        while state.pending.is_none() {
                            worker.wake.wait(&mut state);
                        }
                        let request = state.pending.take().expect("pending request");
                        state.active = Some(request.key.clone());
                        worker.running.store(true, Ordering::Release);
                        request
                    };
                    struct Running<'a>(&'a BackgroundQueue);
                    impl Drop for Running<'_> {
                        fn drop(&mut self) {
                            self.0.state.lock().active = None;
                            self.0.running.store(false, Ordering::Release);
                            self.0.wake.notify_all();
                        }
                    }
                    let _running = Running(&worker);
                    run_background(&worker, &request);
                }
            });
        if let Err(error) = spawned { eprintln!("[motor] preparação em segundo plano adiada: {error}"); return; }
        let inserted = BACKGROUND_QUEUE.set(queue);
        debug_assert!(inserted.is_ok(), "Registry serializes background queue initialization");
        BACKGROUND_QUEUE.get().expect("background queue registered")
    };
    let key = RequestKey {
        source: source_identity(idx),
        config: spec.key.clone(),
        derived: !derived.is_empty(),
    };
    let mut state = queue.state.lock();
    if crate::operations::update_paused() { return; }
    let current = state
        .pending
        .as_ref()
        .map(|r| &r.key)
        .or(state.active.as_ref());
    if current.is_some_and(|current| !supersedes(&key, current)) {
        return;
    }
    // Bound strong cache ownership to this accepted base/desired request,
    // rather than retaining every historical derived variant of one source.
    let keys: HashSet<String> = base.iter().flat_map(|s|&s.parts).chain(&spec.parts).map(|p|p.key.clone()).collect();
    reg.time_cache.retain(|key|keys.contains(key));
    reg.time_attempts.retain(|key,_|key==&spec.key || base.is_some_and(|base|key==&base.key));
    reg.time_attempts.insert(spec.key.clone(),std::time::Instant::now());
    let revision = queue.revision.fetch_add(1, Ordering::SeqCst) + 1;
    state.pending = Some(BackgroundRequest {
        key,
        revision,
        idx: FileIndex {
            parts: idx.parts.clone(),
            lines: Arc::clone(&idx.lines),
            columns: Vec::new(),
            time_order: Arc::clone(&idx.time_order),
        },
        spec: spec.clone(),
        base: base.cloned(),
        derived: derived.to_vec(),
        codes: codes.clone(),
        system: system.clone(),
    });
    queue.wake.notify_one();
}

fn run_background(queue: &BackgroundQueue, request: &BackgroundRequest) {
    let cancelled = || queue.revision.load(Ordering::SeqCst) != request.revision || crate::operations::update_paused();
    let _ = crate::global_scheduler::run(None, crate::global_scheduler::Priority::Background, &cancelled, || run_background_admitted(queue, request));
}
fn run_background_admitted(queue: &BackgroundQueue, request: &BackgroundRequest) {
    let superseded = || queue.revision.load(Ordering::SeqCst) != request.revision;
    for spec in request.base.iter().chain(std::iter::once(&request.spec)) {
        let fields = if request
            .base
            .as_ref()
            .is_some_and(|base| base.key == spec.key)
        {
            &[][..]
        } else {
            &request.derived[..]
        };
        let catalogs = spec
            .baked
            .then(|| (request.codes.clone(), request.system.clone()));
        for part in &spec.parts {
            if superseded() || !still_wanted(&part.key) {
                return;
            }
            if store_ready(part) {
                continue;
            }
            let claimed = with_registry(|reg| {
                if superseded()
                    || !reg.wanted.contains(&part.key)
                    || reg.failed.contains_key(&part.key)
                    || reg.building.contains(&part.key)
                {
                    false
                } else {
                    reg.building.insert(part.key.clone());
                    true
                }
            });
            if !claimed {
                continue;
            }
            let job = Job {
                key: part.key.clone(),
                target: part.path.clone(),
                source: build::copy_part(
                    &request.idx.parts[part.part],
                    Arc::clone(&request.idx.lines),
                    part.start..part.start + part.rows,
                ),
                derived: fields.to_vec(),
                catalogs: catalogs.clone(),
            };
            let phase = Mutex::new(String::from("Preparando checkpoint em segundo plano"));
            let publish = |label: &str, completed: usize| {
                with_registry(|reg| {
                    if superseded() || !reg.wanted.contains(&part.key) {
                        return;
                    }
                    reg.progress.insert(
                        part.key.clone(),
                        BuildProgress {
                            phase: label.into(),
                            completed,
                            total: part.rows,
                            checkpoint_rows: 0,
                            completed_segments: 0,
                            total_segments: 1,
                            resumed_rows: 0,
                            state: "indexing".into(),
                            error: None,
                        },
                    );
                });
            };
            let cancelled = || superseded() || !still_wanted(&part.key);
            publish("Preparando checkpoint em segundo plano", 0);
            let result = run_job(
                job,
                &|n, _| publish(&phase.lock(), n),
                &|label| {
                    *phase.lock() = label.into();
                    publish(label, 0);
                },
                &cancelled,
            );
            let was_cancelled = cancelled();
            let failed = result.is_err();
            with_registry(|reg| {
                reg.building.remove(&part.key);
                reg.progress.remove(&part.key);
                if let Err(error) = result {
                    if !was_cancelled && !superseded() && reg.wanted.contains(&part.key) {
                        reg.failed.insert(part.key.clone(), error.clone());
                        reg.failed.insert(spec.key.clone(), error.clone());
                        reg.failed.insert(request.spec.key.clone(), error);
                    }
                }
            });
            if was_cancelled || failed {
                return;
            }
        }
    }
    prepare_optional_times(queue, request);
}

/// Optional work stays on the mandatory builder's single queue. A foreground
/// interaction pauses it; background status polling is deliberately not a gate.
fn optional_time_enabled() -> bool {
    std::env::var("LOGINSIGHT_TIME_PRECOMPUTE").as_deref() != Ok("0")
}
fn maybe_schedule_time(
    reg: &mut Registry, idx: &FileIndex, spec: &SourceSpec, base: Option<&SourceSpec>,
    derived: &[CompiledDerived], codes: &CodesConfig, system: &CodesConfig,
) {
    if !optional_time_enabled() || crate::operations::update_paused() { return; }
    reg.install_times(spec);
    if spec.parts.iter().all(|part| reg.time_cache.contains(&part.key)) { return; }
    let now = std::time::Instant::now();
    if reg.time_attempts.get(&spec.key).is_some_and(|last| now.duration_since(*last) < std::time::Duration::from_secs(30)) { return; }
    // Retry optional I/O failures at most once per 30 seconds on actual demand.
    reg.time_attempts.insert(spec.key.clone(), now);
    schedule(reg, idx, spec, base, derived, codes, system);
}
fn optional_quiet(cancelled: &dyn Fn() -> bool, quiet: std::time::Duration) -> bool {
    let mut idle = std::time::Instant::now();
    loop {
        if cancelled() || crate::global_scheduler::yield_background(cancelled).is_err() { return false; }
        if crate::operations::interactive_active() { idle = std::time::Instant::now(); }
        else if idle.elapsed() >= quiet { return true; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
fn load_optional_time(part: &PartSpec, cancelled: &(dyn Fn() -> bool + Sync)) -> Result<Option<Arc<time_index::TimeIndex>>, String> {
    if cancelled() { return Err("Preparação temporal pausada.".into()); }
    // The immutable base is pinned before validation and before acquiring the
    // sidecar lease; the returned handle retains its own base/sidecar leases.
    let base = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true)
        .open(part.path.with_extension("build.lock")).map_err(|e| e.to_string())?;
    if fs2::FileExt::try_lock_shared(&base).is_err() { return Ok(None); }
    if !store_ready(part) { return Err("Checkpoint principal indisponível para resumo opcional.".into()); }
    let identity = time_index::identity(&part.path, part.rows)?;
    match time_index::open_cancellable(&part.path, &identity, cancelled) {
        Ok(Some(index)) => return Ok(Some(index)),
        Ok(None) | Err(_) => { if cancelled() { return Err("Preparação temporal pausada.".into()); } }
    }
    let parent = part.path.parent().ok_or("Checkpoint sem diretório.")?;
    let needed = (part.rows as u64).checked_mul(8).and_then(|n| n.checked_add(1 << 30)).ok_or("Resumo temporal grande demais.")?;
    if free_space(parent).is_some_and(|bytes| bytes < needed) { return Err("Resumo temporal adiado por espaço em disco.".into()); }
    let scratch = tempfile::Builder::new().prefix(&format!("{}.time-spill.", part.key)).suffix(".pending")
        .tempdir_in(parent).map_err(|e| e.to_string())?;
    let config = duckdb::Config::default().access_mode(duckdb::AccessMode::ReadOnly).map_err(|e| e.to_string())?;
    let conn = Connection::open_with_flags(&part.path, config).map_err(|e| e.to_string())?;
    conn.execute_batch(&format!("SET temp_directory={}; SET preserve_insertion_order=false;", sql::lit(&scratch.path().to_string_lossy()))).map_err(|e| e.to_string())?;
    limit_resources(&conn, true)?;
    time_index::ensure(&conn, &part.path, &identity, cancelled)?;
    drop(conn);
    if cancelled() { return Err("Preparação temporal pausada.".into()); }
    time_index::open_cancellable(&part.path, &identity, cancelled)
}
fn prepare_optional_times(queue: &BackgroundQueue, request: &BackgroundRequest) {
    if !optional_time_enabled() { return; }
    let obsolete = || queue.revision.load(Ordering::SeqCst) != request.revision || crate::operations::update_paused();
    for spec in request.base.iter().chain(std::iter::once(&request.spec)) {
        for part in &spec.parts {
            if obsolete() || !still_wanted(&part.key) { return; }
            if with_registry(|reg| reg.time_cache.contains(&part.key)) { continue; }
            let mut quiet_ms = 250;
            loop {
                let stopped = || obsolete() || !still_wanted(&part.key);
                if !optional_quiet(&stopped, std::time::Duration::from_millis(quiet_ms)) { return; }
                let preempted = AtomicBool::new(false);
                // Do not acquire Registry while validating/sorting: a foreground
                // Session open may hold it. Revision/UpdatePause are atomic;
                // ownership is checked at each part boundary and publication.
                let cancelled = || {
                    let interactive = crate::operations::interactive_active() || crate::global_scheduler::interactive_waiting();
                    if interactive { preempted.store(true, Ordering::Relaxed); }
                    obsolete() || interactive
                };
                if let Err(error) = crate::sources::validate_source(&request.idx.parts[part.part]) {
                    eprintln!("[motor] resumo temporal adiado: {error}"); return;
                }
                let result = load_optional_time(part, &cancelled);
                if stopped() { return; }
                if preempted.load(Ordering::Relaxed) || crate::operations::interactive_active() {
                    // A preempted optional sort is not a failed source. Wait
                    // for a quiet period, with bounded backoff, then retry it.
                    quiet_ms = (quiet_ms * 2).min(2_000); continue;
                }
                match result {
                    Ok(Some(index)) => {
                        if crate::sources::validate_source(&request.idx.parts[part.part]).is_err() { return; }
                        with_registry(|reg| {
                            if obsolete() || !reg.wanted.contains(&part.key) { return; }
                            reg.time_cache.insert(part.key.clone(), index);
                            reg.install_times(spec);
                        });
                    }
                    Ok(None) => {}
                    Err(error) => eprintln!("[motor] resumo temporal opcional adiado: {error}"),
                }
                break;
            }
        }
    }
}

fn run_job(
    job: Job,
    progress: &(dyn Fn(usize, usize) + Sync),
    phase: &(dyn Fn(&str) + Sync),
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(), String> {
    if cancelled() {
        return Err("Operação cancelada.".into());
    }
    let dir = job
        .target
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(engine_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // Size estimated from the stores built so far (text-heavy logs make
    // stores as large as the file); 1 GB stays free besides.
    let source_len = job.source.lines.range(job.source.range.clone())
        .last()
        .zip(job.source.lines.range(job.source.range.clone()).first())
        .map(|(last, first)| last.offset + u64::from(last.len) - first.offset)
        .unwrap_or(0);
    let needed = source_len / 1000 * STORE_RATIO.load(Ordering::Relaxed) + (1 << 30);
    prune(&job.target);
    if free_space(&dir).is_some_and(|free| free < needed) {
        return Err("Pouco espaço em disco para o índice de consultas rápidas.".into());
    }
    let catalogs = job.catalogs.as_ref().map(|(c, s)| (c, s));
    let target = job.target.clone();
    build::build(
        job.source,
        &job.target,
        &job.derived,
        catalogs,
        progress,
        phase,
        cancelled,
    )?;
    if source_len > 0 {
        let ratio = (build::stored_bytes(&target) * 1100 / source_len).clamp(100, 6000);
        STORE_RATIO.fetch_max(ratio, Ordering::Relaxed);
    }
    Ok(())
}

/// Builds the missing stores now (file loads), reporting progress. Errors
/// leave the source on the line engine.
pub(crate) fn prepare(
    idx: &FileIndex,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<(), String> {
    prepare_detailed(idx, codes, system, derived, &|p| {
        progress(p.completed, p.total)
    })
}

pub(crate) fn prepare_detailed(
    idx: &FileIndex,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
    progress: &(dyn Fn(BuildProgress) + Sync),
) -> Result<(), String> {
    if crate::global_scheduler::current().is_none() {
        return crate::global_scheduler::run(None, crate::global_scheduler::Priority::Normal, &crate::operations::cancelled,
            || prepare_detailed(idx, codes, system, derived, progress))?;
    }
    if crate::operations::update_paused() { return Err("Preparação suspensa para instalar a atualização.".into()); }
    if !enabled() || idx.lines.is_empty() {
        return Ok(());
    }
    // Keep a reusable immutable base even when optional derived definitions
    // change. Only capability-checked operations may choose this base session.
    if !derived.is_empty() {
        prepare_variant(idx, codes, system, &[], progress)?;
    }
    prepare_variant(idx, codes, system, derived, progress)
}

fn prepare_variant(
    idx: &FileIndex,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
    progress: &(dyn Fn(BuildProgress) + Sync),
) -> Result<(), String> {
    for part in &idx.parts {
        crate::sources::validate_source(part)?;
    }
    let Some(spec) = spec(idx, codes, system, derived) else {
        return Ok(());
    };
    with_registry(|reg| {
        reg.remember_spec(idx, &spec);
        reg.wanted.extend(spec.parts.iter().map(|p| p.key.clone()));
    });
    let total: usize = spec.parts.iter().map(|p| p.rows).sum();
    progress(BuildProgress {
        phase: "Validando índices salvos".into(),
        completed: 0,
        total: 0,
        checkpoint_rows: 0,
        completed_segments: 0,
        total_segments: spec.parts.len(),
        resumed_rows: 0,
        state: "indexing".into(),
        error: None,
    });
    let resumed: usize = spec
        .parts
        .iter()
        .filter(|p| store_ready(p))
        .map(|p| p.rows)
        .sum();
    let mut done = resumed;
    let mut segments = spec.parts.iter().filter(|p| store_ready(p)).count();
    let cancellation = crate::operations::current_token();
    let cancelled = || cancellation.cancelled() || crate::operations::update_paused();
    let publish = |phase: &str,
                   completed: usize,
                   checkpoint_rows: usize,
                   completed_segments: usize,
                   state: &str,
                   error: Option<String>| {
        let p = BuildProgress {
            phase: phase.into(),
            completed,
            total,
            checkpoint_rows,
            completed_segments,
            total_segments: spec.parts.len(),
            resumed_rows: resumed,
            state: state.into(),
            error,
        };
        with_registry(|reg| {
            reg.remember_spec(idx, &spec);
            reg.progress.insert(spec.key.clone(), p.clone());
        });
        progress(p);
    };
    publish(
        if resumed == total { "Índices salvos validados" }
        else if resumed > 0 { "Retomando índices; partes ausentes ou inválidas" }
        else { "Preparando índices ausentes ou inválidos" },
        done,
        done,
        segments,
        "indexing",
        None,
    );
    for part in &spec.parts {
        if store_ready(part) {
            continue;
        }
        crate::operations::check()?;
        let claimed = with_registry(|reg| {
            if reg.building.contains(&part.key) {
                false
            } else {
                // A new explicit preparation is a retry; completed segments stay.
                reg.failed.remove(&part.key);
                reg.building.insert(part.key.clone());
                true
            }
        });
        if !claimed {
            continue;
        }
        let job = Job {
            key: part.key.clone(),
            target: part.path.clone(),
            source: build::copy_part(
                &idx.parts[part.part],
                Arc::clone(&idx.lines),
                part.start..part.start + part.rows,
            ),
            derived: derived.to_vec(),
            catalogs: spec.baked.then(|| (codes.clone(), system.clone())),
        };
        let phase_name = Mutex::new(String::from("Convertendo e indexando registros"));
        let produced = std::sync::atomic::AtomicUsize::new(0);
        let result = run_job(
            job,
            &|n, _| {
                produced.store(n, Ordering::Relaxed);
                publish(
                    &phase_name.lock(),
                    done + n,
                    done,
                    segments,
                    "indexing",
                    None,
                );
            },
            &|phase| {
                *phase_name.lock() = phase.to_string();
                publish(
                    phase,
                    done + produced.load(Ordering::Relaxed),
                    done,
                    segments,
                    "indexing",
                    None,
                );
            },
            &cancelled,
        );
        with_registry(|reg| {
            reg.building.remove(&part.key);
            if let Err(error) = &result {
                if !cancelled() {
                    reg.failed.insert(part.key.clone(), error.clone());
                }
            }
        });
        if cancelled() {
            publish(
                "Interrompido; checkpoints concluídos preservados",
                done,
                done,
                segments,
                "cancelled",
                None,
            );
            crate::operations::check()?;
        }
        if let Err(error) = result {
            // A changed source invalidates the line metadata too. It must
            // abort loading, rather than publish a misleading scan fallback.
            for source in &idx.parts {
                crate::sources::validate_source(source)?;
            }
            publish(
                "Motor de linhas ativo; preparação pode ser retomada",
                done,
                done,
                segments,
                "degraded",
                Some(error.clone()),
            );
            eprintln!("[motor] índice não criado: {error}");
            return Ok(());
        }
        done += part.rows;
        segments += 1;
        publish(
            "Checkpoint concluído e validado",
            done,
            done,
            segments,
            "indexing",
            None,
        );
    }
    for source in &idx.parts {
        crate::sources::validate_source(source)?;
    }
    if spec.parts.iter().all(store_ready) {
        publish(
            if resumed == total { "Abrindo índices salvos" } else { "Abrindo índices preparados" },
            done,
            done,
            segments,
            "indexing",
            None,
        );
        let ready = session(idx, codes, system, derived).is_some();
        publish(
            if ready {
                "Consultas prontas"
            } else {
                "Motor de linhas ativo"
            },
            done,
            done,
            segments,
            if ready { "ready" } else { "degraded" },
            None,
        );
    }
    Ok(())
}

/// Store size per 1000 bytes of source, highest seen (starts at 40%).
static STORE_RATIO: AtomicU64 = AtomicU64::new(400);

/// Keeps the store folder bounded: stale builds go first, then the least
/// recently used stores beyond 30 days or the size budget.
fn prune(keep: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(engine_dir()) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let mut stores = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        let modified = meta.modified().unwrap_or(now);
        let age = now.duration_since(modified).unwrap_or_default();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.contains(".pending") {
            if age > std::time::Duration::from_secs(24 * 3600) {
                if let Some(key) = name.split('.').next() {
                    let lock_path = engine_dir().join(format!("{key}.build.lock"));
                    if let Ok(lock) = std::fs::OpenOptions::new()
                        .create(true)
                        .read(true)
                        .write(true)
                        .open(lock_path)
                    {
                        if fs2::FileExt::try_lock_exclusive(&lock).is_ok() {
                            let _ = if meta.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
                        }
                    }
                }
            }
            continue;
        }
        if path.extension().is_some_and(|e| e == "duckdb") && path != keep {
            let used = std::fs::metadata(path.with_extension("used"))
                .ok()
                .and_then(|m| m.modified().ok())
                .unwrap_or(modified);
            stores.push((used, build::stored_bytes(&path).max(meta.len()), path));
        }
    }
    stores.sort_by_key(|(modified, _, _)| *modified);
    let budget: u64 = 8 << 30;
    let mut total: u64 = stores.iter().map(|(_, size, _)| size).sum();
    let in_use: HashSet<PathBuf> = in_use_paths();
    for (modified, size, path) in stores {
        let old = now.duration_since(modified).unwrap_or_default()
            > std::time::Duration::from_secs(30 * 24 * 3600);
        if (old || total > budget) && !in_use.contains(&path) {
            // Never evict another process's active writer or reader. Do not
            // block under the registry lock; a later prune can try again.
            let Ok(lock) = std::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(path.with_extension("build.lock"))
            else {
                continue;
            };
            if fs2::FileExt::try_lock_exclusive(&lock).is_err() {
                continue;
            }
            if build::remove_database(&path) { total = total.saturating_sub(size); }
        }
    }
}

fn in_use_paths() -> HashSet<PathBuf> {
    // The open session and every store of the source in use: pruning them
    // would only make the app build them again, in a loop.
    with_registry(|reg| {
        let mut paths: HashSet<PathBuf> = reg
            .session
            .iter()
            .chain(reg.base_session.iter())
            .flat_map(|s| s.key.split(',').filter_map(|p| p.split('@').next()))
            .map(|k| engine_dir().join(format!("{k}.duckdb")))
            .collect();
        paths.extend(
            reg.wanted
                .iter()
                .map(|k| engine_dir().join(format!("{k}.duckdb"))),
        );
        paths
    })
}

#[cfg(windows)]
fn free_space(dir: &std::path::Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = dir
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free = 0u64;
    unsafe {
        windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            windows::core::PCWSTR(wide.as_ptr()),
            Some(&mut free),
            None,
            None,
        )
        .ok()?;
    }
    Some(free)
}

#[cfg(not(windows))]
fn free_space(dir: &std::path::Path) -> Option<u64> {
    fs2::available_space(dir).ok()
}

#[cfg(test)]
mod segment_tests {
    use super::*;

    #[test]
    fn timestamp_non_null_proof_checks_every_part_and_fails_closed() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(!timestamp_non_null_proof(&conn), "missing relation is not proof");
        conn.execute_batch("CREATE TABLE a(ts BIGINT); CREATE TABLE b(ts BIGINT); CREATE VIEW ev AS SELECT ts FROM a UNION ALL SELECT ts FROM b").unwrap();
        assert!(timestamp_non_null_proof(&conn), "empty ordering is safe");
        conn.execute_batch("INSERT INTO a VALUES (-1), (0), (1); INSERT INTO b VALUES (-7), (0), (7)").unwrap();
        assert!(timestamp_non_null_proof(&conn), "zero and negative values are non-null");
        conn.execute_batch("INSERT INTO b VALUES (NULL)").unwrap();
        assert!(!timestamp_non_null_proof(&conn), "a null in a later part must reject the proof");
        conn.execute_batch("DELETE FROM a; DELETE FROM b; INSERT INTO a VALUES (NULL); INSERT INTO b VALUES (NULL)").unwrap();
        assert!(!timestamp_non_null_proof(&conn), "all-null sources retain null-safe ordering");
    }

    #[test]
    fn segment_bounds_cover_records_exactly_without_splitting_them() {
        let lines = vec![
            crate::model::LineMeta {
                offset: 0,
                len: 4,
                ..Default::default()
            },
            crate::model::LineMeta {
                offset: SEGMENT_BYTES - 1,
                len: 10,
                ..Default::default()
            },
            crate::model::LineMeta {
                offset: SEGMENT_BYTES + 9,
                len: 5,
                ..Default::default()
            },
        ];
        let lines = crate::metadata_store::LineStore::from(lines);
        assert_eq!(segment_ranges(&lines, 0, 3), vec![(0, 2), (2, 3)]);
        assert_eq!(segment_ranges(&lines, 1, 3), vec![(1, 3)]);
        assert!(segment_ranges(&lines, 0, 0).is_empty());
    }
    #[test]
    fn a_single_oversized_record_keeps_one_atomic_range() {
        let lines = vec![
            crate::model::LineMeta {
                offset: 0,
                len: (SEGMENT_BYTES + 1) as u32,
                ..Default::default()
            },
            crate::model::LineMeta {
                offset: SEGMENT_BYTES + 2,
                len: 1,
                ..Default::default()
            },
        ];
        let lines = crate::metadata_store::LineStore::from(lines);
        assert_eq!(segment_ranges(&lines, 0, 2), vec![(0, 1), (1, 2)]);
    }
    #[test]
    fn source_and_derived_configuration_have_separate_checkpoint_keys() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.jsonl");
        std::fs::write(&path, "{\"message\":\"alpha\"}\n").unwrap();
        let old =
            crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let codes = CodesConfig::default();
        let base = spec(&old, &codes, &codes, &[]).unwrap();
        let derived = vec![CompiledDerived {
            name: "extracted".into(),
            source: "message".into(),
            lookup: None,
            steps: Vec::new(),
            rules: vec![crate::sources::CompiledRule::new(regex::Regex::new("(alpha)").unwrap(), None, None).unwrap()],
        }];
        let derived_key = spec(&old, &codes, &codes, &derived).unwrap();
        assert_ne!(base.parts[0].key, derived_key.parts[0].key);
        let mut changed = derived.clone();
        changed[0].name = "other".into();
        assert_ne!(
            derived_key.parts[0].key,
            spec(&old, &codes, &codes, &changed).unwrap().parts[0].key
        );
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"message\":\"beta\"}\n")
            .unwrap();
        assert!(crate::sources::validate_source(&old.parts[0]).is_err());
        let new =
            crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        assert_ne!(
            base.parts[0].key,
            spec(&new, &codes, &codes, &[]).unwrap().parts[0].key
        );
    }
    #[test]
    fn latest_queue_bounds_pending_work_and_keeps_same_source_base_dependency() {
        let key = |config: &str, derived| RequestKey {
            source: "source-a".into(),
            config: config.into(),
            derived,
        };
        let base = key("base", false);
        let full = key("derived-1", true);
        assert!(!supersedes(&full, &full));
        assert!(
            !supersedes(&base, &full),
            "base queries must not displace full build including base"
        );
        let mut pending = Some(full.clone());
        for n in 2..1000 {
            let latest = key(&format!("derived-{n}"), true);
            if pending.as_ref().is_none_or(|old| supersedes(&latest, old)) {
                pending = Some(latest);
            }
        }
        assert_eq!(pending.unwrap().config, "derived-999");
        let other_source = RequestKey {
            source: "source-b".into(),
            config: "base-b".into(),
            derived: false,
        };
        assert!(supersedes(&other_source, &full));
    }
    #[test]
    fn complete_checkpoints_do_not_hide_session_open_failure() {
        let status = finish_status(EngineStatus {
            state: "indexing".into(),
            base_ready: true,
            derived_ready: true,
            phase: "Abrindo consultas".into(),
            completed_rows: 10,
            total_rows: 10,
            completed_segments: 1,
            total_segments: 1,
            resumed_rows: 10,
            can_resume: true,
            error: Some("Falha ao anexar o índice".into()),
        });
        assert_eq!(status.state, "degraded");
        assert!(!status.derived_ready);
        assert!(status.can_resume);
        assert!(status.error.is_some());
    }
    #[test]
    #[cfg(unix)]
    fn atomic_source_replacement_with_same_size_and_mtime_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let content = b"{\"message\":\"alpha\"}\n";
        std::fs::write(&path, content).unwrap();
        let idx =
            crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let replacement = dir.path().join("replacement");
        std::fs::write(&replacement, b"{\"message\":\"omega\"}\n").unwrap();
        std::fs::File::open(&replacement)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        assert!(crate::sources::validate_source(&idx.parts[0]).is_err());
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    fn queue() -> BackgroundQueue {
        BackgroundQueue {
            state: Mutex::new(QueueState::default()),
            wake: parking_lot::Condvar::new(),
            revision: AtomicU64::new(1),
            running: AtomicBool::new(false),
        }
    }

    fn request(dir: &std::path::Path, name: &str, revision: u64) -> BackgroundRequest {
        let path = dir.join(format!("{name}.jsonl"));
        std::fs::write(&path, b"{\"message\":\"alpha\"}\n").unwrap();
        let idx =
            crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let codes = CodesConfig::default();
        let mut spec = spec(&idx, &codes, &codes, &[]).unwrap();
        // Any accidental build remains isolated to the fixture, not user data.
        for part in &mut spec.parts {
            part.path = dir.join(format!("{}.duckdb", part.key));
        }
        BackgroundRequest {
            key: RequestKey {
                source: source_identity(&idx),
                config: spec.key.clone(),
                derived: false,
            },
            revision,
            idx,
            spec,
            base: None,
            derived: Vec::new(),
            codes: CodesConfig::default(),
            system: CodesConfig::default(),
        }
    }

    fn leased_session(path: &std::path::Path) -> Arc<Session> {
        let lease = std::fs::File::create(path).unwrap();
        fs2::FileExt::try_lock_shared(&lease).unwrap();
        Arc::new(Session {
            key: path.to_string_lossy().into_owned(),
            base: Mutex::new(Connection::open_in_memory().unwrap()),
            pool: Mutex::new(Vec::new()),
            schema: sql::Schema::default(),
            timestamps_non_null: false,
            baked: false,
            names: RwLock::new(None),
            names_version: AtomicU64::new(0),
            selections: Mutex::new(Vec::new()),
            selection_builds: Mutex::new(HashSet::new()),
            selection_changed: parking_lot::Condvar::new(),
            garbage: Arc::new(Mutex::new(Vec::new())),
            row_count: None,
            texts: Vec::new(),
            time_indexes: RwLock::new(None),
            _leases: vec![lease],
        })
    }

    fn failed_preparation_survives_publication(previous_source: bool) {
        let dir = tempfile::tempdir().unwrap();
        let incoming = request(dir.path(), "incoming", 1);
        let old = request(dir.path(), "previous", 1);
        let error = "Pouco espaço em disco para o índice de consultas rápidas.";
        let part_key = incoming.spec.parts[0].key.clone();
        let mut reg = Registry::default();
        if previous_source {
            reg.source_identity = old.key.source.clone();
            reg.remember_spec(&old.idx, &old.spec);
            reg.wanted.insert(old.spec.parts[0].key.clone());
            reg.failed
                .insert(old.spec.key.clone(), "old failure".into());
            reg.base_session = Some(leased_session(&dir.path().join("old.lock")));
        }
        let old_session = reg.base_session.as_ref().map(Arc::downgrade);
        // This is the state recorded by prepare_variant's degraded return:
        // a valid line index, failed segment and progress, but no new session.
        reg.remember_spec(&incoming.idx, &incoming.spec);
        reg.wanted.insert(part_key.clone());
        reg.failed.insert(part_key.clone(), error.into());
        reg.progress.insert(
            incoming.spec.key.clone(),
            BuildProgress {
                phase: "Motor de linhas ativo; preparação pode ser retomada".into(),
                completed: 0,
                total: incoming.idx.lines.len(),
                checkpoint_rows: 0,
                completed_segments: 0,
                total_segments: incoming.spec.parts.len(),
                resumed_rows: 0,
                state: "degraded".into(),
                error: Some(error.into()),
            },
        );
        reg.source_published(Some(&incoming.idx), None);
        assert_eq!(reg.source_identity, incoming.key.source);
        assert_eq!(reg.failed.get(&part_key).map(String::as_str), Some(error));
        let progress = &reg.progress[&incoming.spec.key];
        assert_eq!(progress.state, "degraded");
        assert_eq!(progress.error.as_deref(), Some(error));
        assert_eq!(reg.wanted, HashSet::from([part_key]));
        assert!(!reg.failed.contains_key(&old.spec.key));
        assert!(old_session.is_none_or(|weak| weak.upgrade().is_none()));
        assert_eq!(reg.source_keys.len(), 1);
        assert!(reg.source_keys.contains_key(&incoming.key.source));
    }

    #[test]
    fn first_source_publication_preserves_its_failed_preparation_and_retry_cause() {
        failed_preparation_survives_publication(false);
    }

    #[test]
    fn replacement_publication_preserves_incoming_failure_and_releases_previous_source() {
        failed_preparation_survives_publication(true);
    }

    #[test]
    fn merged_source_publication_preserves_constituent_segment_failure() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = request(dir.path(), "first", 1);
        let second = request(dir.path(), "second", 1);
        let failed_key = second.spec.parts[0].key.clone();
        let mut reg = Registry {
            source_identity: first.key.source.clone(),
            ..Default::default()
        };
        reg.remember_spec(&first.idx, &first.spec);
        reg.remember_spec(&second.idx, &second.spec);
        reg.wanted
            .extend([first.spec.parts[0].key.clone(), failed_key.clone()]);
        reg.failed.insert(failed_key.clone(), "disk full".into());
        first.idx.append(second.idx).unwrap();
        let codes = CodesConfig::default();
        let merged = spec(&first.idx, &codes, &codes, &[]).unwrap();
        assert_eq!(merged.parts[1].key, failed_key);
        reg.source_published(Some(&first.idx), None);
        assert_eq!(
            reg.failed.get(&merged.parts[1].key).map(String::as_str),
            Some("disk full")
        );
        assert!(reg.wanted.contains(&failed_key));
        assert!(reg.wanted.contains(&merged.parts[0].key));
        assert_eq!(reg.source_identity, source_identity(&first.idx));
    }

    #[test]
    fn successful_session_adoption_does_not_leave_previous_source_ownership_behind() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = Registry::default();
        for n in 0..32 {
            let current = request(dir.path(), &format!("source-{n}"), 1);
            reg.remember_spec(&current.idx, &current.spec);
            // Session::open succeeds before the application publishes it.
            reg.source_identity = current.key.source.clone();
            reg.source_published(Some(&current.idx), None);
            assert_eq!(reg.source_keys.len(), 1);
            assert!(reg.source_keys.contains_key(&current.key.source));
        }
    }

    #[test]
    fn clearing_or_replacing_source_releases_sessions_without_killing_active_readers() {
        for next in [None, Some("replacement")] {
            let dir = tempfile::tempdir().unwrap();
            let base_path = dir.path().join("base.lock");
            let derived_path = dir.path().join("derived.lock");
            let base = leased_session(&base_path);
            let derived = leased_session(&derived_path);
            let base_weak = Arc::downgrade(&base);
            let derived_weak = Arc::downgrade(&derived);
            let active_reader = Arc::clone(&base);
            let mut reg = Registry {
                source_identity: "previous".into(),
                base_session: Some(base),
                session: Some(derived),
                wanted: HashSet::from(["checkpoint".into()]),
                building: HashSet::from(["checkpoint".into()]),
                failed: HashMap::from([("old-error".into(), "error".into())]),
                ..Default::default()
            };
            reg.retain_source(next, &HashSet::new());
            assert!(reg.wanted.is_empty());
            assert!(reg.failed.is_empty());
            assert_eq!(reg.source_identity, next.unwrap_or_default());
            assert!(
                reg.building.contains("checkpoint"),
                "builder owns claim until unwind"
            );
            assert!(derived_weak.upgrade().is_none());
            assert!(
                base_weak.upgrade().is_some(),
                "in-flight query retains its lease"
            );
            let derived_lock = std::fs::File::open(&derived_path).unwrap();
            fs2::FileExt::try_lock_exclusive(&derived_lock).unwrap();
            let base_lock = std::fs::File::open(&base_path).unwrap();
            assert!(fs2::FileExt::try_lock_exclusive(&base_lock).is_err());
            drop(active_reader);
            assert!(base_weak.upgrade().is_none());
            fs2::FileExt::try_lock_exclusive(&base_lock).unwrap();
        }
    }

    #[test]
    fn clearing_source_cancels_active_request_and_releases_metadata_when_it_unwinds() {
        let dir = tempfile::tempdir().unwrap();
        let active = request(dir.path(), "active", 1);
        let lines = Arc::downgrade(&active.idx.lines);
        let mapping = Arc::downgrade(&active.idx.parts[0].mmap);
        let queue = queue();
        queue.state.lock().active = Some(active.key.clone());
        queue.retain_source(None);
        assert_ne!(queue.revision.load(Ordering::SeqCst), active.revision);
        assert!(queue.state.lock().active.is_none());
        // A cancelled worker must return before touching stores or claiming
        // more segments, even if an old wanted key remains somewhere else.
        run_background(&queue, &active);
        assert!(!active.spec.parts[0].path.exists());
        assert!(lines.upgrade().is_some());
        drop(active);
        assert!(lines.upgrade().is_none());
        assert!(mapping.upgrade().is_none());
    }

    #[test]
    fn clearing_source_drops_pending_metadata_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let pending = request(dir.path(), "pending", 1);
        let lines = Arc::downgrade(&pending.idx.lines);
        let mapping = Arc::downgrade(&pending.idx.parts[0].mmap);
        let queue = queue();
        queue.state.lock().pending = Some(pending);
        queue.retain_source(None);
        assert!(queue.state.lock().pending.is_none());
        assert!(lines.upgrade().is_none());
        assert!(mapping.upgrade().is_none());
    }

    #[test]
    fn publishing_prepared_source_preserves_matching_sessions_and_active_build() {
        let dir = tempfile::tempdir().unwrap();
        let active = request(dir.path(), "active", 1);
        let session = leased_session(&dir.path().join("prepared.lock"));
        let mut reg = Registry {
            source_identity: active.key.source.clone(),
            base_session: Some(Arc::clone(&session)),
            wanted: HashSet::from([active.spec.parts[0].key.clone()]),
            ..Default::default()
        };
        let queue = queue();
        queue.state.lock().active = Some(active.key.clone());
        reg.remember_spec(&active.idx, &active.spec);
        reg.source_published(Some(&active.idx), Some(&queue));
        assert!(Arc::ptr_eq(reg.base_session.as_ref().unwrap(), &session));
        assert!(reg.wanted.contains(&active.spec.parts[0].key));
        assert_eq!(queue.revision.load(Ordering::SeqCst), active.revision);
        assert_eq!(queue.state.lock().active.as_ref(), Some(&active.key));
    }

    #[test]
    fn publishing_new_source_keeps_its_pending_work_while_old_active_work_stops() {
        let dir = tempfile::tempdir().unwrap();
        let old = request(dir.path(), "old", 1);
        let new = request(dir.path(), "new", 2);
        let new_key = new.key.clone();
        let queue = queue();
        queue.revision.store(2, Ordering::SeqCst);
        *queue.state.lock() = QueueState {
            active: Some(old.key.clone()),
            pending: Some(new),
        };
        queue.retain_source(Some(&new_key.source));
        let state = queue.state.lock();
        assert!(state.active.is_none());
        assert_eq!(state.pending.as_ref().map(|r| &r.key), Some(&new_key));
        assert_eq!(queue.revision.load(Ordering::SeqCst), 2);
        assert_ne!(queue.revision.load(Ordering::SeqCst), old.revision);
    }

    #[test]
    fn publishing_empty_index_releases_old_source_without_starting_a_worker() {
        let dir = tempfile::tempdir().unwrap();
        let pending = request(dir.path(), "pending", 1);
        let lines = Arc::downgrade(&pending.idx.lines);
        let mut empty = request(dir.path(), "empty", 1).idx;
        Arc::make_mut(&mut empty.lines).clear();
        let session = leased_session(&dir.path().join("old.lock"));
        let weak_session = Arc::downgrade(&session);
        let mut reg = Registry {
            source_identity: pending.key.source.clone(),
            base_session: Some(session),
            wanted: HashSet::from([pending.spec.parts[0].key.clone()]),
            ..Default::default()
        };
        let queue = queue();
        queue.state.lock().pending = Some(pending);
        reg.source_published(Some(&empty), Some(&queue));
        assert!(reg.source_identity.is_empty());
        assert!(reg.wanted.is_empty());
        assert!(weak_session.upgrade().is_none());
        assert!(lines.upgrade().is_none());
        assert!(queue.state.lock().pending.is_none());
        assert_eq!(queue.revision.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn removing_a_superseding_request_does_not_resurrect_or_deduplicate_old_work() {
        let dir = tempfile::tempdir().unwrap();
        let old = request(dir.path(), "old", 1);
        let new = request(dir.path(), "new", 2);
        let queue = queue();
        queue.revision.store(2, Ordering::SeqCst);
        *queue.state.lock() = QueueState {
            active: Some(old.key.clone()),
            pending: Some(new),
        };
        queue.retain_source(Some(&old.key.source));
        let state = queue.state.lock();
        assert!(state.pending.is_none());
        assert!(
            state.active.is_none(),
            "a fresh request must not deduplicate cancelled work"
        );
        assert_ne!(queue.revision.load(Ordering::SeqCst), old.revision);
    }
}

#[cfg(test)]
mod metadata_identity_tests {
    use super::*;

    /// Key without Java enrichment, optionally retaining the independent web
    /// parser revision. Tiny fixtures each have one segment.
    fn pre_java_key(idx: &FileIndex, include_web_revision: bool) -> String {
        let part = &idx.parts[0];
        assert!(part.custom.is_none() && part.ts_config.is_none());
        let first_offset = idx.lines.at(0).offset - part.base;
        let last = idx.lines.last().unwrap();
        let last_end = last.offset - part.base + u64::from(last.len);
        let derived_sig = derived_signature(&[]).unwrap();
        let tz = chrono::Local::now().offset().to_string();
        let custom = ""; let ts = ""; let catalogs = "";
        let mut hash = Sha256::new();
        hash.update(format!(
            "{}|{}|{}|{}|{:?}|{custom}|{ts}|{tz}|{}|{first_offset}|{last_end}|{derived_sig}|{catalogs}|{:?}",
            build::STORE_VERSION, crate::index_cache::INDEX_DIR, part.identity,
            part.format, part.header, idx.lines.len(), part.physical_file_id
        ));
        hash.update(b"|timezone-configuration:");
        hash.update(part.calendar.timezone.as_bytes());
        if matches!(part.format.as_str(), "syslog3164" | "firewall") {
            hash.update(format!("|inferred-year:{}", part.calendar.year));
        }
        if let Some(identity) = &part.event_identity {
            hash.update(b"|logical-event-identity:");
            hash.update(serde_json::to_vec(identity).unwrap());
        }
        if include_web_revision {
            if let Some(revision) = crate::sources::parser_semantics_signature(&part.format) {
                hash.update(b"|structured-parser:");
                hash.update(revision.as_bytes());
            }
        }
        format!("{:x}", hash.finalize())
    }

    #[test]
    fn java_enrichment_changes_only_java_store_keys_not_plain_nginx_or_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let codes = CodesConfig::default();
        for (format, raw, java) in [
            ("jsonl", "{\"timestamp\":\"2026-09-30T00:00:00Z\",\"message\":\"ordinary\"}\n", false),
            ("apache", "127.0.0.1 - - [30/Sep/2026:12:00:00 +0000] \"GET /health HTTP/1.1\" 200 5 \"-\" \"client\"\n", false),
            ("log4j", "2026-09-30 12:00:00,000 ERROR [worker] a.Logger - a.FailureException\n\tat a.Service.run(A.java:1)\n2026-09-30 12:00:01,000 INFO [worker] a.Logger - next\n", true),
            ("wildfly", "12:00:00,000 ERROR [a.Logger] (worker) a.FailureException\n\tat a.Service.run(A.java:1)\n12:00:01,000 INFO [a.Logger] (worker) next\n", true),
        ] {
            let path = dir.path().join(format!("{format}.log"));
            std::fs::write(&path, raw).unwrap();
            let idx = crate::sources::index_file(path.to_str().unwrap(), format, None, None, None).unwrap();
            let actual = spec(&idx, &codes, &codes, &[]).unwrap();
            assert_eq!(actual.parts.len(), 1);
            let old = pre_java_key(&idx, true);
            assert_eq!(actual.parts[0].key == old, !java, "{format}");
            if java {
                assert_eq!(idx.lines.len(), 2, "enrichment preserves the existing two framed records");
                for &column in crate::java_stacktrace::COLUMNS {
                    assert!(idx.columns.iter().any(|value| value == column));
                }
            } else {
                assert!(!idx.columns.iter().any(|value| value == "java.trace"));
            }
        }
    }

    #[test]
    fn structured_parser_revision_refreshes_affected_stores_without_changing_record_identity() {
        let dir = tempfile::tempdir().unwrap();
        let codes = CodesConfig::default();
        for (format, raw, affected) in [
            ("jsonl", "{\"time_iso8601\":\"2026-10-02T12:00:00Z\",\"request\":\"GET /health HTTP/1.1\",\"status\":200}\n", true),
            ("apache", "127.0.0.1 - - [02/Oct/2026:12:00:00 +0000] \"GET /health HTTP/1.1\" 200 5 \"-\" \"client\"\n", true),
            ("nginx-error", "2026/10/02 12:00:00 [error] 7#7: *12 open() failed, client: 127.0.0.1, server: example.test\n", true),
            ("logfmt", "time=2026-10-02T12:00:00Z level=info msg=healthy request_id=42\n", true),
            ("mixed", "127.0.0.1 - - [02/Oct/2026:12:00:00 +0000] \"-\" 400 0\n{\"message\":\"next\"}\n", true),
            ("text", "127.0.0.1 - - [02/Oct/2026:12:00:00 +0000] \"GET / HTTP/1.1\" 200 5\n", false),
            ("syslog3164", "Oct  2 12:00:00 host app: healthy\n", false),
            ("csv", "timestamp,message\n2026-10-02T12:00:00Z,healthy\n", false),
            ("custom", "explicit user format remains authoritative\n", false),
        ] {
            let path = dir.path().join(format!("{format}.log"));
            std::fs::write(&path, raw).unwrap();
            let idx = crate::sources::index_file(path.to_str().unwrap(), format, None, None, None).unwrap();
            let identity = idx.parts[0].identity.clone();
            let logical_identity = idx.parts[0].event_identity.clone();
            let event_ref = crate::sources::event_at(&idx, 0, &codes, &codes, &[]).event_ref;
            assert_eq!(event_ref, format!("{}:{}", logical_identity.as_deref().unwrap_or(&identity), idx.lines.at(0).offset - idx.parts[0].base));
            let actual = spec(&idx, &codes, &codes, &[]).unwrap();
            assert_eq!(actual.parts.len(), 1);
            assert_eq!(actual.parts[0].key != pre_java_key(&idx, false), affected, "{format}");
            assert_eq!(actual.parts[0].key, pre_java_key(&idx, true));
            assert_eq!(idx.parts[0].identity, identity);
            assert_eq!(idx.parts[0].event_identity, logical_identity);
            assert_eq!(crate::sources::event_at(&idx, 0, &codes, &codes, &[]).event_ref, event_ref);
        }
    }

    #[test]
    fn timezone_context_separates_new_engine_keys_from_unverifiable_legacy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("calendar.jsonl");
        std::fs::write(&path, "{\"timestamp\":\"2026-09-30T00:00:00Z\",\"message\":\"alpha\"}\n").unwrap();
        let mut idx = crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let codes = CodesConfig::default();
        let actual = spec(&idx, &codes, &codes, &[]).unwrap().parts[0].key.clone();
        let part = &idx.parts[0];
        let first_offset = idx.lines.at(0).offset - part.base;
        let last = idx.lines.last().unwrap();
        let last_end = last.offset - part.base + u64::from(last.len);
        let custom = ""; let ts = ""; let catalogs = "";
        let tz = chrono::Local::now().offset().to_string();
        let derived_sig = derived_signature(&[]).unwrap();
        let mut hash = Sha256::new();
        hash.update(format!(
            "{}|{}|{}|{}|{:?}|{custom}|{ts}|{tz}|{}|{first_offset}|{last_end}|{derived_sig}|{catalogs}|{:?}",
            build::STORE_VERSION, "indexes-v6", part.identity,
            part.format, part.header, idx.lines.len(), part.physical_file_id
        ));
        assert_ne!(actual, format!("{:x}", hash.finalize()));
        let original_zone = idx.parts[0].calendar.timezone.clone();
        idx.parts[0].calendar.timezone = "different-historical-rules-with-the-same-current-offset".into();
        assert_ne!(actual, spec(&idx, &codes, &codes, &[]).unwrap().parts[0].key);
        idx.parts[0].calendar.timezone = original_zone;
        idx.parts[0].calendar.year += 1;
        assert_eq!(actual, spec(&idx, &codes, &codes, &[]).unwrap().parts[0].key);
        idx.parts[0].event_identity = Some("stable-logical-source".into());
        assert_ne!(actual, spec(&idx, &codes, &codes, &[]).unwrap().parts[0].key);
    }

    #[test]
    fn inferred_year_and_logical_identity_separate_engine_variants() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("syslog.log");
        std::fs::write(&path, "Sep 30 12:00:00 host app: alpha\n").unwrap();
        let mut idx = crate::sources::index_file(path.to_str().unwrap(), "syslog3164", None, None, None).unwrap();
        let codes = CodesConfig::default();
        let first = spec(&idx, &codes, &codes, &[]).unwrap().parts[0].key.clone();
        idx.parts[0].calendar.year += 1;
        let next = spec(&idx, &codes, &codes, &[]).unwrap().parts[0].key.clone();
        assert_ne!(first, next);
        idx.parts[0].event_identity = Some("original:event".into());
        let alias = spec(&idx, &codes, &codes, &[]).unwrap().parts[0].key.clone();
        assert_ne!(next, alias);
        idx.parts[0].event_identity = Some("other:event".into());
        assert_ne!(alias, spec(&idx, &codes, &codes, &[]).unwrap().parts[0].key);
    }
    #[test]
    fn optional_worker_yields_to_interaction_and_quiesce_cancellation() {
        let interacting = crate::operations::interactive();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancelled);
        let (done, result) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            done.send(optional_quiet(&|| stop.load(Ordering::Acquire), std::time::Duration::from_millis(1))).unwrap();
        });
        assert!(result.recv_timeout(std::time::Duration::from_millis(60)).is_err(), "optional work must not start during an interaction");
        cancelled.store(true, Ordering::Release);
        assert!(!result.recv_timeout(std::time::Duration::from_secs(2)).unwrap());
        worker.join().unwrap();
        drop(interacting);
        assert!(optional_quiet(&|| false, std::time::Duration::from_millis(1)), "idle retry is allowed without a new scheduler");
    }

}

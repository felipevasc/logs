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
mod sql;
mod text;
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
    let mut text = String::new();
    for d in derived {
        text.push_str(&format!("{}\u{1}{}\u{1}", d.name, d.source));
        for rule in &d.rules {
            // A rule reading the line position would depend on file order.
            if let Some(filter) = &rule.filter {
                if filter.column == "id" || (filter.op == "query" && query_reads_id(&filter.value))
                {
                    return None;
                }
            }
            text.push_str(&format!(
                "{}\u{2}{:?}\u{2}{}\u{3}",
                rule.re.as_str(),
                rule.template,
                rule.filter
                    .as_ref()
                    .map(|f| serde_json::to_string(f).unwrap_or_default())
                    .unwrap_or_default()
            ));
        }
        if d.source == "id" {
            return None;
        }
    }
    Some(text)
}

fn query_reads_id(text: &str) -> bool {
    match crate::querylang::compile(text) {
        Ok(expr) => {
            let mut names = Vec::new();
            expr.field_names(&mut names);
            names.iter().any(|n| n == "id")
        }
        Err(_) => true,
    }
}

fn catalogs_signature(codes: &CodesConfig, system: &CodesConfig) -> String {
    static CACHE: Mutex<Option<((u64, usize, usize, usize, usize), String)>> = Mutex::new(None);
    let key = catalog_key(codes, system);
    if let Some((cached, sig)) = &*CACHE.lock() {
        if *cached == key {
            return sig.clone();
        }
    }
    let mut hash = Sha256::new();
    for catalog in [codes, system] {
        let mut sources: Vec<_> = catalog.sources.iter().collect();
        sources.sort_by(|a, b| a.0.cmp(b.0));
        for (source, entries) in sources {
            let mut entries: Vec<_> = entries.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (code, info) in entries {
                hash.update(format!(
                    "{source}\u{1}{code}\u{1}{}\u{1}{}\u{2}",
                    info.name, info.description
                ));
            }
        }
        hash.update([3u8]);
    }
    let sig = format!("{:x}", hash.finalize());
    *CACHE.lock() = Some((key, sig.clone()));
    sig
}

fn catalog_key(codes: &CodesConfig, system: &CodesConfig) -> (u64, usize, usize, usize, usize) {
    (
        CATALOG_EPOCH.load(Ordering::SeqCst),
        codes as *const _ as usize,
        system as *const _ as usize,
        codes.sources.values().map(|m| m.len()).sum(),
        system.sources.values().map(|m| m.len()).sum(),
    )
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
    lines: &[crate::model::LineMeta],
    start: usize,
    end: usize,
) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = start;
    while from < end {
        let limit = (from + SEGMENT_ROWS).min(end);
        let byte_end = lines[from].offset.saturating_add(SEGMENT_BYTES);
        let count = lines[from..limit].partition_point(|line| line.offset < byte_end);
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
            let first_offset = idx.lines[from].offset - part.base;
            let last = &idx.lines[to - 1];
            let last_end = last.offset - part.base + u64::from(last.len);
            let mut hash = Sha256::new();
            hash.update(format!(
                "{}|{}|{}|{}|{:?}|{custom}|{ts}|{tz}|{}|{first_offset}|{last_end}|{derived_sig}|{catalogs}|{:?}",
                build::STORE_VERSION, crate::index_cache::INDEX_DIR, part.identity,
                part.format, part.header, to - from, part.physical_file_id
            ));
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
    baked: bool,
    /// Catalog fingerprint loaded into `enr`; readers hold it while querying names.
    names: RwLock<Option<(u64, usize, usize, usize, usize)>>,
    /// Bumped whenever `enr` is reloaded (part of selection keys).
    names_version: AtomicU64,
    /// Recent selections of costly filters, most recent first.
    selections: Mutex<Vec<(String, Arc<ops::Selection>)>>,
    /// Selection tables no longer referenced, dropped before new ones are made.
    garbage: Arc<Mutex<Vec<String>>>,
    /// Inverted text index of each part with its first line; empty when a
    /// part has none (free text is then scanned).
    texts: Vec<(usize, text::Text)>,
    /// Shared OS leases outlive database connections and mapped text readers.
    _leases: Vec<std::fs::File>,
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

impl Session {
    fn open(spec: &SourceSpec) -> Result<Session, String> {
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
        limit_resources(&conn);
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
            "CREATE TABLE enr (source VARCHAR, code VARCHAR, name VARCHAR, description VARCHAR); \
             CREATE VIEW evn AS SELECT ev.*, COALESCE(enr.name, ev.pname) AS name, \
             COALESCE(enr.description, ev.pdesc) AS description FROM ev \
             LEFT JOIN enr ON enr.source = ev.source AND enr.code = ev.code;"
                .to_string()
        };
        conn.execute_batch(&format!("CREATE VIEW ev AS {union}; {names_view}"))
            .map_err(|e| e.to_string())?;
        Ok(Session {
            key: spec.key.clone(),
            base: Mutex::new(conn),
            pool: Mutex::new(Vec::new()),
            schema,
            baked,
            names: RwLock::new(None),
            names_version: AtomicU64::new(0),
            selections: Mutex::new(Vec::new()),
            garbage: Arc::new(Mutex::new(Vec::new())),
            texts,
            _leases: leases,
        })
    }

    /// Lines (sorted) whose free text may contain `needle`, through the
    /// inverted indexes; `None` when they cannot narrow the search.
    pub(crate) fn free_candidates(&self, needle: &str, limit: usize) -> Option<Vec<usize>> {
        if self.texts.is_empty() {
            return None;
        }
        let mut out = Vec::new();
        for (start, text) in &self.texts {
            let lids = text.candidates(needle, limit.checked_sub(out.len())?)?;
            out.extend(lids.into_iter().map(|lid| start + lid as usize));
        }
        Some(out)
    }

    pub(crate) fn names_version(&self) -> u64 {
        self.names_version.load(Ordering::SeqCst)
    }

    pub(crate) fn cached_selection(&self, key: &str) -> Option<Arc<ops::Selection>> {
        let mut selections = self.selections.lock();
        let at = selections.iter().position(|(k, _)| k == key)?;
        let entry = selections.remove(at);
        let found = Arc::clone(&entry.1);
        selections.insert(0, entry);
        Some(found)
    }

    pub(crate) fn cache_selection(&self, key: String, selection: Arc<ops::Selection>) {
        let mut selections = self.selections.lock();
        selections.retain(|(k, _)| *k != key);
        selections.insert(0, (key, selection));
        selections.truncate(8);
    }

    pub(crate) fn garbage(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.garbage)
    }

    pub(crate) fn take_garbage(&self) -> Vec<String> {
        std::mem::take(&mut *self.garbage.lock())
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

    /// Loads the catalogs' names for the codes present, when they changed.
    fn refresh_names(&self, codes: &CodesConfig, system: &CodesConfig) -> Result<(), String> {
        if self.baked {
            return Ok(());
        }
        let key = catalog_key(codes, system);
        if *self.names.read() == Some(key) {
            return Ok(());
        }
        let mut current = self.names.write();
        if *current == Some(key) {
            return Ok(());
        }
        let conn = self.conn()?;
        conn.execute_batch("DELETE FROM enr")
            .map_err(|e| e.to_string())?;
        let pairs: Vec<(String, String)> = {
            let mut stmt = conn
                .prepare("SELECT DISTINCT source, code FROM ev WHERE code <> ''")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?
        };
        {
            let mut appender = conn.appender("enr").map_err(|e| e.to_string())?;
            for (source, code) in pairs {
                if let Some(info) = codes
                    .lookup(&source, &code)
                    .or_else(|| system.lookup(&source, &code))
                {
                    appender
                        .append_row(duckdb::params![source, code, info.name, info.description])
                        .map_err(|e| e.to_string())?;
                }
            }
            appender.flush().map_err(|e| e.to_string())?;
        }
        *current = Some(key);
        self.names_version.fetch_add(1, Ordering::SeqCst);
        self.selections.lock().clear();
        Ok(())
    }

    /// Holds the names table steady while a statement reads it.
    pub(crate) fn names_guard(
        &self,
    ) -> parking_lot::RwLockReadGuard<'_, Option<(u64, usize, usize, usize, usize)>> {
        self.names.read()
    }
}

/// DuckDB would use every core and 80% of the memory; a desktop app shares
/// the machine. The engine gets the shared worker budget and 40% of the
/// installed memory, spilling larger intermediate results to disk.
pub(crate) fn limit_resources(conn: &Connection) {
    let megabytes = crate::resources::duckdb_memory_mb();
    let _ = conn.execute_batch(&format!(
        "SET threads = {}; SET memory_limit = '{megabytes}MB';",
        crate::resources::query_threads()
    ));
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
    progress: HashMap<String, BuildProgress>,
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
pub(crate) fn session(
    idx: &FileIndex,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> Option<Arc<Session>> {
    if !enabled() || idx.lines.is_empty() {
        return None;
    }
    let base = if derived.is_empty() {
        None
    } else {
        spec(idx, codes, system, &[])
    };
    let spec = spec(idx, codes, system, derived)?;
    if let Some(error) = idx
        .parts
        .iter()
        .find_map(|part| crate::sources::validate_source(part).err())
    {
        with_registry(|reg| {
            reg.failed.insert(spec.key.clone(), error);
        });
        return None;
    }
    let session = with_registry(|reg| -> Option<Arc<Session>> {
        let identity = source_identity(idx);
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
                return Some(Arc::clone(current));
            }
        }
        if reg.failed.contains_key(&spec.key) {
            return None;
        }
        let missing: Vec<&PartSpec> = spec.parts.iter().filter(|p| !store_ready(p)).collect();
        if !missing.is_empty() {
            if missing
                .iter()
                .any(|part| !reg.failed.contains_key(&part.key))
            {
                schedule(idx, &spec, base.as_ref(), derived, codes, system);
            }
            return None;
        }
        match Session::open(&spec) {
            Ok(session) => {
                let session = Arc::new(session);
                if derived.is_empty() {
                    reg.base_session = Some(Arc::clone(&session));
                } else {
                    reg.session = Some(Arc::clone(&session));
                }
                Some(session)
            }
            Err(error) => {
                eprintln!("[motor] sessão indisponível: {error}");
                if !error.starts_with("Checkpoint ocupado") {
                    reg.failed.insert(spec.key.clone(), error);
                }
                None
            }
        }
    })?;
    match session.refresh_names(codes, system) {
        Ok(()) => Some(session),
        Err(error) => {
            eprintln!("[motor] nomes indisponíveis: {error}");
            None
        }
    }
}

struct Job {
    key: String,
    target: PathBuf,
    source: build::PartSource,
    derived: Vec<CompiledDerived>,
    catalogs: Option<(CodesConfig, CodesConfig)>,
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
        schedule(idx, &desired, base.as_ref(), derived, codes, system);
    });
}

fn source_identity(idx: &FileIndex) -> String {
    idx.parts
        .iter()
        .map(|p| {
            format!(
                "{}|{}|{:?}|{}",
                p.identity,
                p.format,
                p.physical_file_id,
                p.ts_config
                    .as_ref()
                    .map(|c| c.signature())
                    .unwrap_or_default()
            )
        })
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
}

fn supersedes(new: &RequestKey, current: &RequestKey) -> bool {
    new != current && !(new.source == current.source && !new.derived && current.derived)
}

fn schedule(
    idx: &FileIndex,
    spec: &SourceSpec,
    base: Option<&SourceSpec>,
    derived: &[CompiledDerived],
    codes: &CodesConfig,
    system: &CodesConfig,
) {
    static QUEUE: std::sync::OnceLock<Arc<BackgroundQueue>> = std::sync::OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        let queue = Arc::new(BackgroundQueue {
            state: Mutex::new(QueueState::default()),
            wake: parking_lot::Condvar::new(),
            revision: AtomicU64::new(0),
        });
        let worker = Arc::clone(&queue);
        std::thread::Builder::new()
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
                        request
                    };
                    run_background(&worker, &request);
                    // Never hold the queue mutex while acquiring the registry.
                    worker.state.lock().active = None;
                }
            })
            .expect("engine worker thread");
        queue
    });
    let key = RequestKey {
        source: source_identity(idx),
        config: spec.key.clone(),
        derived: !derived.is_empty(),
    };
    let mut state = queue.state.lock();
    let current = state
        .pending
        .as_ref()
        .map(|r| &r.key)
        .or(state.active.as_ref());
    if current.is_some_and(|current| !supersedes(&key, current)) {
        return;
    }
    let revision = queue.revision.fetch_add(1, Ordering::SeqCst) + 1;
    state.pending = Some(BackgroundRequest {
        key,
        revision,
        idx: FileIndex {
            parts: idx.parts.clone(),
            lines: Arc::clone(&idx.lines),
            columns: Vec::new(),
            time_order: std::sync::OnceLock::new(),
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
                if reg.failed.contains_key(&part.key) || reg.building.contains(&part.key) {
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
                    if !was_cancelled {
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
    let source_len = job.source.lines[job.source.range.clone()]
        .last()
        .zip(job.source.lines[job.source.range.clone()].first())
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
    with_registry(|reg| reg.wanted.extend(spec.parts.iter().map(|p| p.key.clone())));
    let total: usize = spec.parts.iter().map(|p| p.rows).sum();
    progress(BuildProgress {
        phase: "Verificando integridade dos checkpoints".into(),
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
    let cancelled = || cancellation.cancelled();
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
            reg.progress.insert(spec.key.clone(), p.clone());
        });
        progress(p);
    };
    publish(
        "Validando e reutilizando checkpoints",
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
            "Abrindo consultas preparadas",
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
                            let _ = std::fs::remove_file(&path);
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
            build::remove_database(&path);
            total = total.saturating_sub(size);
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
            rules: vec![crate::sources::CompiledRule {
                re: regex::Regex::new("(alpha)").unwrap(),
                template: None,
                filter: None,
            }],
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

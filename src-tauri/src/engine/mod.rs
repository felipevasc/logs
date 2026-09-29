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
                if filter.column == "id" || (filter.op == "query" && query_reads_id(&filter.value)) {
                    return None;
                }
            }
            text.push_str(&format!(
                "{}\u{2}{:?}\u{2}{}\u{3}",
                rule.re.as_str(),
                rule.template,
                rule.filter.as_ref().map(|f| serde_json::to_string(f).unwrap_or_default()).unwrap_or_default()
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
                hash.update(format!("{source}\u{1}{code}\u{1}{}\u{1}{}\u{2}", info.name, info.description));
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

struct PartSpec {
    key: String,
    path: PathBuf,
    start: usize,
    rows: usize,
    part: usize,
    identity: String,
}

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
        let end = idx.lines.partition_point(|m| m.offset < part.base + part.mmap.len() as u64);
        if start != next || end < start {
            return None;
        }
        ranges.push((start, end));
        next = end;
    }
    (next == idx.lines.len()).then_some(ranges)
}

fn spec(idx: &FileIndex, codes: &CodesConfig, system: &CodesConfig, derived: &[CompiledDerived]) -> Option<SourceSpec> {
    let derived_sig = derived_signature(derived)?;
    let baked = !derived.is_empty();
    let catalogs = if baked { catalogs_signature(codes, system) } else { String::new() };
    let tz = chrono::Local::now().offset().to_string();
    let dir = engine_dir();
    let mut parts = Vec::with_capacity(idx.parts.len());
    for (k, (part, (start, end))) in idx.parts.iter().zip(part_ranges(idx)?).enumerate() {
        let custom = match &part.custom {
            Some(crate::sources::CustomParse::Regex(r)) => format!("re:{}", r.as_str()),
            Some(crate::sources::CustomParse::Delimited { sep, fields }) => format!("dl:{sep}{fields:?}"),
            None => String::new(),
        };
        let ts = part
            .ts_config
            .as_ref()
            .map(|c| {
                let rules: Vec<(Option<&str>, Option<&String>)> =
                    c.rules.iter().map(|(re, tpl)| (re.as_ref().map(|r| r.as_str()), tpl.as_ref())).collect();
                format!(
                    "{:?}|{}|{:?}|{:?}|{}|{:?}",
                    c.sources, c.format, c.complement, c.timezone_offset_minutes, c.clock_adjustment_ms, rules
                )
            })
            .unwrap_or_default();
        let mut hash = Sha256::new();
        hash.update(format!(
            "{}|{}|{}|{}|{:?}|{custom}|{ts}|{tz}|{}|{derived_sig}|{catalogs}",
            build::STORE_VERSION,
            crate::index_cache::INDEX_DIR,
            part.identity,
            part.format,
            part.header,
            end - start
        ));
        let key = format!("{:x}", hash.finalize());
        parts.push(PartSpec {
            path: dir.join(format!("{key}.duckdb")),
            key,
            start,
            rows: end - start,
            part: k,
            identity: part.identity.clone(),
        });
    }
    let key = parts.iter().map(|p| format!("{}@{}", p.key, p.start)).collect::<Vec<_>>().join(",");
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
        let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
        let temp = engine_dir().join("tmp");
        let _ = std::fs::create_dir_all(&temp);
        conn.execute_batch(&format!(
            "SET temp_directory = {}; SET preserve_insertion_order = false;",
            sql::lit(&temp.to_string_lossy())
        ))
        .map_err(|e| e.to_string())?;
        limit_memory(&conn);
        udf::register(&conn).map_err(|e| e.to_string())?;
        let mut schema = sql::Schema::default();
        let mut columns: HashMap<String, String> = HashMap::new();
        let mut overflow: HashSet<String> = HashSet::new();
        let mut selects = Vec::new();
        let mut baked = spec.baked;
        for (k, part) in spec.parts.iter().enumerate() {
            // Touch the store (before it is opened) so the cache keeps recently used ones.
            let _ = std::fs::File::options()
                .append(true)
                .open(&part.path)
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
                let global = columns.entry(field.clone()).or_insert_with(|| format!("w{next}"));
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
            let spilled = overflow.contains(name).then(|| format!("xv[list_position(xk, {})]", sql::lit(name)));
            let expr = match (wide, spilled) {
                (Some(w), Some(o)) => format!("COALESCE({w}, {o})"),
                (Some(w), None) => w.clone(),
                (None, Some(o)) => o,
                (None, None) => continue,
            };
            schema.fields.insert(name.clone(), expr);
            schema.lower.entry(name.to_ascii_lowercase()).or_default().push(name.clone());
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
        })
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
        Ok(Pooled { session: self, conn: Some(conn) })
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
        conn.execute_batch("DELETE FROM enr").map_err(|e| e.to_string())?;
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
                if let Some(info) = codes.lookup(&source, &code).or_else(|| system.lookup(&source, &code)) {
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
    pub(crate) fn names_guard(&self) -> parking_lot::RwLockReadGuard<'_, Option<(u64, usize, usize, usize, usize)>> {
        self.names.read()
    }
}

/// DuckDB may use 80% of the memory by default; a desktop app shares it.
/// Half of that default (40% of the memory) is left to the engine, which
/// spills larger intermediate results to its temporary folder.
pub(crate) fn limit_memory(conn: &Connection) {
    let Ok(default) = conn.query_row("SELECT current_setting('memory_limit')", [], |r| r.get::<_, String>(0)) else {
        return;
    };
    let mut parts = default.split_whitespace();
    let (Some(number), Some(unit)) = (parts.next(), parts.next()) else { return };
    let Ok(number) = number.parse::<f64>() else { return };
    let scale = match unit {
        "KiB" | "KB" => 1u64 << 10,
        "MiB" | "MB" => 1 << 20,
        "GiB" | "GB" => 1 << 30,
        "TiB" | "TB" => 1 << 40,
        _ => return,
    };
    let megabytes = ((number * scale as f64 / 2.0) as u64 >> 20).max(512);
    let _ = conn.execute_batch(&format!("SET memory_limit = '{megabytes}MB'"));
}

// ---------------------------------------------------------------- registry

#[derive(Default)]
struct Registry {
    session: Option<Arc<Session>>,
    building: HashSet<String>,
    failed: HashMap<String, String>,
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
    let spec = spec(idx, codes, system, derived)?;
    let session = with_registry(|reg| -> Option<Arc<Session>> {
        if let Some(current) = &reg.session {
            if current.key == spec.key {
                return Some(Arc::clone(current));
            }
        }
        if reg.failed.contains_key(&spec.key) {
            return None;
        }
        let missing: Vec<&PartSpec> = spec.parts.iter().filter(|p| !p.path.exists()).collect();
        if !missing.is_empty() {
            for part in missing {
                if reg.building.contains(&part.key) || reg.failed.contains_key(&part.key) {
                    continue;
                }
                reg.building.insert(part.key.clone());
                schedule(idx, part, derived, spec.baked.then(|| (codes.clone(), system.clone())));
            }
            return None;
        }
        match Session::open(&spec) {
            Ok(session) => {
                let session = Arc::new(session);
                reg.session = Some(Arc::clone(&session));
                Some(session)
            }
            Err(error) => {
                eprintln!("[motor] sessão indisponível: {error}");
                reg.failed.insert(spec.key.clone(), error);
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

fn schedule(idx: &FileIndex, part: &PartSpec, derived: &[CompiledDerived], catalogs: Option<(CodesConfig, CodesConfig)>) {
    static QUEUE: Mutex<Option<std::sync::mpsc::Sender<Job>>> = parking_lot::const_mutex(None);
    let job = Job {
        key: part.key.clone(),
        target: part.path.clone(),
        source: build::copy_part(&idx.parts[part.part], &idx.lines[part.start..part.start + part.rows]),
        derived: derived.to_vec(),
        catalogs,
    };
    let mut queue = QUEUE.lock();
    let sender = queue.get_or_insert_with(|| {
        let (sender, receiver) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("loginsight-engine".into())
            .spawn(move || {
                for job in receiver {
                    let key = job.key.clone();
                    let result = run_job(job, &|_, _| {}, &|| false);
                    with_registry(|reg| {
                        reg.building.remove(&key);
                        if let Err(error) = result {
                            eprintln!("[motor] índice não criado: {error}");
                            reg.failed.insert(key, error);
                        }
                    });
                }
            })
            .expect("engine worker thread");
        sender
    });
    let _ = sender.send(job);
}

fn run_job(
    job: Job,
    progress: &(dyn Fn(usize, usize) + Sync),
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(), String> {
    if job.target.exists() {
        return Ok(());
    }
    let dir = job.target.parent().map(PathBuf::from).unwrap_or_else(engine_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // Stores take about a third of the file; keep 1 GB free besides.
    let needed = job.source.part.mmap.len() as u64 / 5 * 2 + (1 << 30);
    if free_space(&dir).is_some_and(|free| free < needed) {
        return Err("Pouco espaço em disco para o índice de consultas rápidas.".into());
    }
    prune(&job.target);
    let catalogs = job.catalogs.as_ref().map(|(c, s)| (c, s));
    build::build(job.source, &job.target, &job.derived, catalogs, progress, cancelled).map(|_| ())
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
    if !enabled() || idx.lines.is_empty() {
        return Ok(());
    }
    let Some(spec) = spec(idx, codes, system, derived) else { return Ok(()) };
    let total: usize = spec.parts.iter().filter(|p| !p.path.exists()).map(|p| p.rows).sum();
    let mut done = 0;
    for part in &spec.parts {
        if part.path.exists() {
            continue;
        }
        let claimed = with_registry(|reg| {
            if reg.building.contains(&part.key) || reg.failed.contains_key(&part.key) {
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
            source: build::copy_part(&idx.parts[part.part], &idx.lines[part.start..part.start + part.rows]),
            derived: derived.to_vec(),
            catalogs: spec.baked.then(|| (codes.clone(), system.clone())),
        };
        let offset = done;
        let result = run_job(job, &|n, _| progress(offset + n, total), &crate::operations::cancelled);
        with_registry(|reg| {
            reg.building.remove(&part.key);
            if let Err(error) = &result {
                if !crate::operations::cancelled() {
                    reg.failed.insert(part.key.clone(), error.clone());
                }
            }
        });
        crate::operations::check()?;
        if let Err(error) = result {
            eprintln!("[motor] índice não criado: {error}");
            return Ok(());
        }
        done += part.rows;
    }
    // Opening now keeps the first query fast.
    let _ = session(idx, codes, system, derived);
    Ok(())
}

/// Keeps the store folder bounded: stale builds go first, then the least
/// recently used stores beyond 30 days or the size budget.
fn prune(keep: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(engine_dir()) else { return };
    let now = std::time::SystemTime::now();
    let mut stores = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        let modified = meta.modified().unwrap_or(now);
        let age = now.duration_since(modified).unwrap_or_default();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.contains(".pending") {
            if age > std::time::Duration::from_secs(24 * 3600) {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        }
        if path.extension().is_some_and(|e| e == "duckdb") && path != keep {
            stores.push((modified, meta.len(), path));
        }
    }
    stores.sort_by_key(|(modified, _, _)| *modified);
    let budget: u64 = 8 << 30;
    let mut total: u64 = stores.iter().map(|(_, size, _)| size).sum();
    let in_use: HashSet<PathBuf> = in_use_paths();
    for (modified, size, path) in stores {
        let old = now.duration_since(modified).unwrap_or_default() > std::time::Duration::from_secs(30 * 24 * 3600);
        if (old || total > budget) && !in_use.contains(&path) {
            build::remove_database(&path);
            total = total.saturating_sub(size);
        }
    }
}

fn in_use_paths() -> HashSet<PathBuf> {
    with_registry(|reg| {
        reg.session
            .as_ref()
            .map(|s| s.key.split(',').filter_map(|p| p.split('@').next()).map(|k| engine_dir().join(format!("{k}.duckdb"))).collect())
            .unwrap_or_default()
    })
}

#[cfg(windows)]
fn free_space(dir: &std::path::Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
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
fn free_space(_dir: &std::path::Path) -> Option<u64> {
    None
}

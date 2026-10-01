//! Builds the columnar store of one file part from the very events the app
//! materializes (`event_at`), so every column holds the value the line
//! engine would compute.
use crate::entities::{self, ROLES};
use crate::model::{label_class, CodesConfig, Event, LV_OTHER};
use crate::sources::{event_at, CompiledDerived, FileIndex, FilePart};
use duckdb::arrow::array::{
    new_null_array, ArrayRef, Int64Array, ListBuilder, StringArray, StringBuilder, UInt32Array,
    UInt64Array, UInt8Array,
};
use duckdb::arrow::datatypes::{DataType, Field, Schema};
use duckdb::arrow::record_batch::RecordBatch;
use duckdb::Connection;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Store layout version; part of every store key.
pub(crate) const STORE_VERSION: u32 = 5;
/// Lines sampled to choose which fields become their own columns.
const SAMPLE: usize = 20_000;
/// Fields beyond this many live in the overflow lists (still exact, slower).
const MAX_WIDE: usize = 1_024;
/// Separator of the lowercase free-text values (`vals`).
pub(crate) const SEP: char = '\u{1}';

/// Role columns: `@role` values as `Event::col_ref` returns them, plus the
/// search language's tool (which also reads command lines from messages).
pub(crate) fn role_column(column: &str) -> String {
    format!("c_{}", column.trim_start_matches('@'))
}
pub(crate) const QUERY_TOOL: &str = "q_tool";

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct StoreInfo {
    pub version: u32,
    pub rows: usize,
    /// Field name and its column.
    pub wide: Vec<(String, String)>,
    /// Fields stored in the `xk`/`xv` lists.
    pub overflow: Vec<String>,
    /// Fields with object or array values (dotted paths can reach into them).
    pub structured: Vec<String>,
    /// ASCII-lowercase names shared by two keys of one record.
    pub ci_multi: Vec<String>,
    /// Names and descriptions were resolved while building (derived fields exist).
    pub baked: bool,
}

/// The completion marker is written last. A database without its matching
/// marker (or text store) is an interrupted build, never a usable checkpoint.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
struct Manifest {
    version: u32,
    key: String,
    identity: String,
    rows: usize,
    first_offset: u64,
    last_end: u64,
    database_bytes: u64,
    artifacts: Vec<Artifact>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
struct Artifact {
    name: String,
    bytes: u64,
    sha256: String,
}

fn manifest_path(path: &Path) -> PathBuf {
    path.with_extension("complete.json")
}
fn digest(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut input = std::io::BufReader::with_capacity(
        1 << 20,
        std::fs::File::open(path).map_err(|e| e.to_string())?,
    );
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        crate::operations::check()?;
        let count = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn artifacts(path: &Path) -> Result<Vec<Artifact>, String> {
    let mut out = vec![Artifact {
        name: "database".into(),
        bytes: std::fs::metadata(path).map_err(|e| e.to_string())?.len(),
        sha256: digest(path)?,
    }];
    let text = super::text::dir_of(path);
    let mut files: Vec<_> = std::fs::read_dir(&text)
        .map_err(|e| e.to_string())?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    files.sort_by_key(|f| f.file_name());
    for file in files {
        let name = file.file_name().to_string_lossy().into_owned();
        if name.ends_with(".lock") {
            continue;
        }
        if !file.file_type().map_err(|e| e.to_string())?.is_file() {
            continue;
        }
        out.push(Artifact {
            name,
            bytes: file.metadata().map_err(|e| e.to_string())?.len(),
            sha256: digest(&file.path())?,
        });
    }
    Ok(out)
}
// Each immutable checkpoint is checksum-verified once per process. Replacing or
// truncating any artifact invalidates this memo through the metadata signature.
type Stamp = Vec<(String, u64, Option<std::time::SystemTime>)>;
static VERIFIED: std::sync::LazyLock<
    parking_lot::Mutex<std::collections::HashMap<PathBuf, Stamp>>,
> = std::sync::LazyLock::new(|| parking_lot::Mutex::new(std::collections::HashMap::new()));

pub(crate) fn published_matches(
    path: &Path,
    identity: &str,
    rows: usize,
    first_offset: u64,
    last_end: u64,
) -> bool {
    let Ok(bytes) = std::fs::read(manifest_path(path)) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_slice::<Manifest>(&bytes) else {
        return false;
    };
    if manifest.version != STORE_VERSION
        || path.file_stem().and_then(|s| s.to_str()) != Some(manifest.key.as_str())
        || manifest.identity != identity
        || manifest.rows != rows
        || manifest.first_offset != first_offset
        || manifest.last_end != last_end
        || !std::fs::metadata(path).is_ok_and(|m| m.len() > 0 && m.len() == manifest.database_bytes)
        || !manifest.artifacts.iter().any(|a| a.name == "database")
        || !manifest.artifacts.iter().any(|a| a.name == "meta.json")
    {
        return false;
    }
    let text = super::text::dir_of(path);
    let mut stamp = Vec::with_capacity(manifest.artifacts.len() + 1);
    let Ok(meta) = std::fs::metadata(manifest_path(path)) else {
        return false;
    };
    stamp.push(("manifest".into(), meta.len(), meta.modified().ok()));
    for artifact in &manifest.artifacts {
        if artifact.name != "database"
            && (artifact.name.contains(['/', '\\']) || artifact.name == "..")
        {
            return false;
        }
        let file = if artifact.name == "database" {
            path.to_path_buf()
        } else {
            text.join(&artifact.name)
        };
        let Ok(meta) = std::fs::metadata(file) else {
            return false;
        };
        if meta.len() != artifact.bytes {
            return false;
        }
        stamp.push((artifact.name.clone(), meta.len(), meta.modified().ok()));
    }
    if VERIFIED.lock().get(path).is_some_and(|old| old == &stamp) {
        return true;
    }
    let Ok(actual) = artifacts(path) else {
        return false;
    };
    if actual != manifest.artifacts {
        return false;
    }
    let Ok(conn) = Connection::open(path) else {
        return false;
    };
    if super::limit_resources(&conn, true).is_err() { return false; }
    if !read_info(&conn, "main").is_ok_and(|info| info.rows == rows) {
        return false;
    }
    let shape: std::result::Result<(u64, Option<u32>, Option<u32>), _> =
        conn.query_row("SELECT count(*), min(lid), max(lid) FROM ev", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        });
    if !shape.is_ok_and(|(count, min, max)| {
        count == rows as u64 && min == Some(0) && max == Some(rows.saturating_sub(1) as u32)
    }) {
        return false;
    }
    let Ok(index) = tantivy::Index::open_in_dir(&text) else {
        return false;
    };
    let Ok(reader) = index.reader() else {
        return false;
    };
    if reader.searcher().num_docs() != rows as u64 {
        return false;
    }
    VERIFIED.lock().insert(path.to_path_buf(), stamp);
    true
}

pub(crate) fn stored_bytes(path: &Path) -> u64 {
    let auxiliary = std::fs::metadata(super::time_index::path(path)).map(|m|m.len()).unwrap_or(0);
    let primary: u64 = std::fs::read(manifest_path(path))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok())
        .map(|m| m.artifacts.iter().map(|a| a.bytes).sum())
        .unwrap_or(0);
    primary.saturating_add(auxiliary)
}

fn publish_manifest(path: &Path, manifest: &Manifest) -> Result<(), String> {
    use std::io::Write;
    let target = manifest_path(path);
    let temporary = target.with_extension(format!("{}.pending", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::File::create(&temporary).map_err(|e| e.to_string())?;
        let bytes = serde_json::to_vec(manifest).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        std::fs::rename(&temporary, &target).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        if let Some(parent) = target.parent() {
            std::fs::File::open(parent)
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

/// Copy of a part that stays valid after the app's locks are released.
pub(crate) struct PartSource {
    pub part: FilePart,
    pub lines: Arc<crate::metadata_store::LineStore>,
    pub range: std::ops::Range<usize>,
}

pub(crate) fn copy_part(
    part: &FilePart,
    lines: Arc<crate::metadata_store::LineStore>,
    range: std::ops::Range<usize>,
) -> PartSource {
    PartSource {
        part: FilePart {
            path: part.path.clone(),
            physical_path: part.physical_path.clone(),
            physical_file_id: part.physical_file_id,
            calendar: part.calendar.clone(),
            event_identity: part.event_identity.clone(),
            metadata_identity: part.metadata_identity.clone(),
            canonical_lease: part.canonical_lease.clone(),
            file_name: part.file_name.clone(),
            format: part.format.clone(),
            custom: part.custom.clone(),
            ts_config: part.ts_config.clone(),
            header: part.header.clone(),
            mmap: Arc::clone(&part.mmap),
            base: part.base,
            identity: part.identity.clone(),
        },
        lines,
        range,
    }
}

struct Row {
    ts: Option<i64>,
    lvl: u8,
    level: String,
    source: String,
    code: String,
    message: String,
    pname: String,
    pdesc: String,
    eref: Option<String>,
    off: u64,
    vals: String,
    pat: String,
    roles: Vec<Option<String>>,
    wide: Vec<(usize, String)>,
    over: Vec<(String, String)>,
    structured: Vec<String>,
    ci_multi: Vec<String>,
}

/// Text of a field exactly as `Event::col_str` gives it.
fn field_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Free-text values as `querylang::any_value` visits them, lowercased; names
/// and descriptions come from the catalogs at query time.
pub(crate) fn free_text(ev: &Event) -> String {
    let mut out = String::with_capacity(ev.message.len() + ev.source.len() + 64);
    let mut push = |text: &str| {
        if !out.is_empty() {
            out.push(SEP);
        }
        out.push_str(&text.to_lowercase());
    };
    push(&ev.message);
    push(&ev.source);
    push(&ev.code);
    for (key, value) in &ev.fields {
        if matches!(key.as_str(), "arquivo" | "caminho") {
            continue;
        }
        match value {
            Value::String(s) => push(s),
            Value::Number(n) => push(&n.to_string()),
            Value::Bool(b) => push(if *b { "true" } else { "false" }),
            Value::Array(_) | Value::Object(_) => push(&value.to_string()),
            Value::Null => {}
        }
    }
    out
}

fn row_of(ev: Event, off: u64, identity: &str, wide: &HashMap<String, usize>) -> Row {
    let extracted = entities::extract(&ev);
    let mut roles: Vec<Option<String>> = ROLES
        .iter()
        .map(|info| match info.role {
            // `Event::col_ref("@tool")` reads tools from fields only.
            entities::Role::Tool => entities::tool(&ev).map(|t| t.name.to_string()),
            role => extracted.get(role).map(str::to_string),
        })
        .collect();
    roles.push(extracted.get(entities::Role::Tool).map(str::to_string));
    drop(extracted);
    let mut lower_keys: HashMap<String, usize> = HashMap::new();
    let mut ci_multi = Vec::new();
    let mut structured = Vec::new();
    let mut wide_values = Vec::new();
    let mut over = Vec::new();
    for (key, value) in &ev.fields {
        let lower = key.to_ascii_lowercase();
        let seen = lower_keys.entry(lower.clone()).or_default();
        *seen += 1;
        if *seen == 2 {
            ci_multi.push(lower);
        }
        if matches!(value, Value::Array(_) | Value::Object(_)) {
            structured.push(key.clone());
        }
        match wide.get(key) {
            Some(&column) => wide_values.push((column, field_text(value))),
            None => over.push((key.clone(), field_text(value))),
        }
    }
    let default_ref = format!("{identity}:{off}");
    Row {
        ts: ev.timestamp,
        lvl: label_class(&ev.level).unwrap_or(LV_OTHER),
        vals: free_text(&ev),
        pat: crate::insights::pattern_of(&ev.message),
        eref: (ev.event_ref != default_ref).then_some(ev.event_ref),
        level: ev.level,
        source: ev.source,
        code: ev.code,
        message: ev.message,
        pname: ev.name,
        pdesc: ev.description,
        off,
        roles,
        wide: wide_values,
        over,
        structured,
        ci_multi,
    }
}

fn strings<'a>(values: impl Iterator<Item = Option<&'a str>>) -> ArrayRef {
    Arc::new(values.collect::<StringArray>())
}

/// Arrow schema of the `ev` table, in the column order of [`create_table`].
fn table_schema(wide: usize) -> Arc<Schema> {
    let text = |name: &str| Field::new(name, DataType::Utf8, true);
    let list = |name: &str| {
        Field::new(
            name,
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            true,
        )
    };
    let mut fields = vec![
        Field::new("lid", DataType::UInt32, true),
        Field::new("ts", DataType::Int64, true),
        Field::new("lvl", DataType::UInt8, true),
        text("level"),
        text("source"),
        text("code"),
        text("message"),
        text("pname"),
        text("pdesc"),
        text("eref"),
        Field::new("off", DataType::UInt64, true),
        text("vals"),
        text("pat"),
    ];
    fields.extend(ROLES.iter().map(|info| text(&role_column(info.column))));
    fields.push(text(QUERY_TOOL));
    fields.extend((0..wide).map(|i| text(&format!("f{i}"))));
    fields.push(list("xk"));
    fields.push(list("xv"));
    Arc::new(Schema::new(fields))
}

/// One batch with every column of the table; absent columns are all NULL.
fn batch_of(rows: &[Row], first: u32, wide: usize, schema: &Arc<Schema>) -> RecordBatch {
    let n = rows.len();
    let optional = |values: Vec<Option<&str>>| -> ArrayRef {
        if values.iter().all(Option::is_none) {
            new_null_array(&DataType::Utf8, n)
        } else {
            strings(values.into_iter())
        }
    };
    let mut arrays: Vec<ArrayRef> = vec![
        Arc::new(UInt32Array::from_iter_values(
            (0..n as u32).map(|r| first + r),
        )),
        Arc::new(rows.iter().map(|r| r.ts).collect::<Int64Array>()),
        Arc::new(UInt8Array::from_iter_values(rows.iter().map(|r| r.lvl))),
        strings(rows.iter().map(|r| Some(r.level.as_str()))),
        strings(rows.iter().map(|r| Some(r.source.as_str()))),
        strings(rows.iter().map(|r| Some(r.code.as_str()))),
        strings(rows.iter().map(|r| Some(r.message.as_str()))),
        strings(rows.iter().map(|r| Some(r.pname.as_str()))),
        strings(rows.iter().map(|r| Some(r.pdesc.as_str()))),
        optional(rows.iter().map(|r| r.eref.as_deref()).collect()),
        Arc::new(UInt64Array::from_iter_values(rows.iter().map(|r| r.off))),
        strings(rows.iter().map(|r| Some(r.vals.as_str()))),
        strings(rows.iter().map(|r| Some(r.pat.as_str()))),
    ];
    for slot in 0..=ROLES.len() {
        arrays.push(optional(
            rows.iter().map(|r| r.roles[slot].as_deref()).collect(),
        ));
    }
    let mut columns: Vec<Option<Vec<Option<&str>>>> = vec![None; wide];
    for (r, row) in rows.iter().enumerate() {
        for (column, value) in &row.wide {
            columns[*column].get_or_insert_with(|| vec![None; n])[r] = Some(value.as_str());
        }
    }
    for values in columns {
        arrays.push(match values {
            Some(values) => strings(values.into_iter()),
            None => new_null_array(&DataType::Utf8, n),
        });
    }
    if rows.iter().any(|r| !r.over.is_empty()) {
        let mut keys = ListBuilder::new(StringBuilder::new());
        let mut values = ListBuilder::new(StringBuilder::new());
        for row in rows {
            if row.over.is_empty() {
                keys.append_null();
                values.append_null();
                continue;
            }
            for (key, value) in &row.over {
                keys.values().append_value(key);
                values.values().append_value(value);
            }
            keys.append(true);
            values.append(true);
        }
        arrays.push(Arc::new(keys.finish()));
        arrays.push(Arc::new(values.finish()));
    } else {
        let list = schema.field(schema.fields().len() - 1).data_type().clone();
        arrays.push(new_null_array(&list, n));
        arrays.push(new_null_array(&list, n));
    }
    RecordBatch::try_new(Arc::clone(schema), arrays).expect("batch matches the table schema")
}

fn create_table(conn: &Connection, wide_names: &[String]) -> duckdb::Result<()> {
    let mut sql = String::from(
        "CREATE TABLE ev (lid UINTEGER NOT NULL, ts BIGINT, lvl UTINYINT, level VARCHAR, source VARCHAR, \
         code VARCHAR, message VARCHAR, pname VARCHAR, pdesc VARCHAR, eref VARCHAR, off UBIGINT, \
         vals VARCHAR, pat VARCHAR",
    );
    for info in ROLES {
        sql.push_str(&format!(", {} VARCHAR", role_column(info.column)));
    }
    sql.push_str(&format!(", {QUERY_TOOL} VARCHAR"));
    for name in wide_names {
        sql.push_str(&format!(", {name} VARCHAR"));
    }
    sql.push_str(", xk VARCHAR[], xv VARCHAR[]); CREATE TABLE meta (k VARCHAR, v VARCHAR);");
    conn.execute_batch(&sql)
}

/// Picks the fields that get their own column from a spread-out sample.
fn choose_wide(
    idx: &FileIndex,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> Vec<String> {
    let n = idx.lines.len();
    let head = n.min(SAMPLE / 2);
    let mut positions: Vec<usize> = (0..head).collect();
    let spread = (n - head).min(SAMPLE / 2);
    positions.extend((0..spread).map(|k| head + k * (n - head) / spread.max(1)));
    let mut sampled_bytes = 0usize;
    positions.retain(|&i| {
        sampled_bytes = sampled_bytes.saturating_add(idx.lines.at(i).len as usize);
        sampled_bytes <= crate::resources::batch_bytes().max(1 << 20)
    });
    let seen: Vec<Vec<String>> = positions
        .par_iter()
        .map(|&i| {
            event_at(idx, i, codes, system, derived)
                .fields
                .keys()
                .cloned()
                .collect()
        })
        .collect();
    let mut order: Vec<String> = Vec::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for keys in seen {
        for key in keys {
            let count = counts.entry(key.clone()).or_default();
            if *count == 0 {
                order.push(key);
            }
            *count += 1;
        }
    }
    let rank: HashMap<&String, usize> = order.iter().enumerate().map(|(i, k)| (k, i)).collect();
    let mut chosen: Vec<&String> = order.iter().collect();
    chosen.sort_by(|a, b| counts[*b].cmp(&counts[*a]).then(rank[a].cmp(&rank[b])));
    chosen.truncate(MAX_WIDE);
    let keep: HashSet<&String> = chosen.into_iter().collect();
    order.iter().filter(|k| keep.contains(k)).cloned().collect()
}

pub(crate) struct Outcome {
    #[allow(dead_code)]
    pub info: StoreInfo,
}

/// Builds one resumable store at `target`, reporting processed records and
/// finalization phases. Only the last, validated completion marker makes it
/// queryable. Failure removes the active segment's data; older segments survive.
pub(crate) fn build(
    source: PartSource,
    target: &Path,
    derived: &[CompiledDerived],
    catalogs: Option<(&CodesConfig, &CodesConfig)>,
    progress: &(dyn Fn(usize, usize) + Sync),
    phase: &(dyn Fn(&str) + Sync),
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Outcome, String> {
    use fs2::FileExt;
    crate::sources::validate_source(&source.part)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(target.with_extension("build.lock"))
        .map_err(|e| e.to_string())?;
    let waiting = std::time::Instant::now();
    loop {
        match lock.try_lock_exclusive() {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if cancelled() {
                    return Err("Operação cancelada.".into());
                }
                if waiting.elapsed() >= std::time::Duration::from_secs(5) {
                    return Err("Outro processo está preparando este checkpoint; tente novamente após ele concluir.".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(error) => return Err(format!("Não foi possível bloquear o checkpoint: {error}")),
        }
    }
    let segment = &source.lines.range(source.range.clone());
    let first_offset = segment
        .first()
        .map(|m| m.offset - source.part.base)
        .unwrap_or(0);
    let last_end = segment
        .last()
        .map(|m| m.offset - source.part.base + u64::from(m.len))
        .unwrap_or(0);
    let mut manifest = Manifest {
        version: STORE_VERSION,
        key: target
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string(),
        identity: source.part.identity.clone(),
        rows: segment.len(),
        first_offset,
        last_end,
        database_bytes: 0,
        artifacts: Vec::new(),
    };
    if published_matches(
        target,
        &manifest.identity,
        manifest.rows,
        first_offset,
        last_end,
    ) {
        let conn = Connection::open(target).map_err(|e| e.to_string())?;
        return read_info(&conn, "main").map(|info| Outcome { info });
    }
    // Unique scratch names also prevent collisions between windows in one process.
    let pending = target.with_extension(format!("{}.pending", uuid::Uuid::new_v4()));
    let source_check = source.part.clone();
    let result = write(
        source, &pending, derived, catalogs, progress, phase, cancelled,
    );
    match result {
        Ok(outcome) => {
            if let Err(error) = crate::sources::validate_source(&source_check) {
                remove_database(&pending);
                return Err(error);
            }
            if cancelled() {
                remove_database(&pending);
                return Err("Operação cancelada.".into());
            }
            if published_matches(
                target,
                &manifest.identity,
                manifest.rows,
                first_offset,
                last_end,
            ) {
                remove_database(&pending);
                return Ok(outcome);
            }
            phase("Publicando checkpoint validado");
            // Incomplete artifacts may remain after process termination. Only a
            // validated marker allows reuse; replace all artifacts as one unit.
            remove_database(target);
            let result = (|| {
                std::fs::rename(super::text::dir_of(&pending), super::text::dir_of(target))
                    .map_err(|e| e.to_string())?;
                std::fs::rename(&pending, target).map_err(|e| e.to_string())?;
                manifest.database_bytes =
                    std::fs::metadata(target).map_err(|e| e.to_string())?.len();
                manifest.artifacts = artifacts(target)?;
                crate::sources::validate_source(&source_check)?;
                publish_manifest(target, &manifest)
            })();
            if let Err(error) = result {
                remove_database(&pending);
                remove_database(target);
                return Err(format!("Não foi possível publicar o checkpoint: {error}"));
            }
            Ok(outcome)
        }
        Err(error) => {
            remove_database(&pending);
            Err(error)
        }
    }
}

pub(crate) fn remove_database(path: &Path) -> bool {
    // Published targets are removed under their exclusive build.lock; pending
    // artifacts stay under their parent's writer lease. Keep the complete
    // publication intact if its auxiliary lease cannot close.
    if let Err(error) = super::time_index::remove_under_build_lock(path) {
        eprintln!("[motor] limpeza adiada: {error}"); return false;
    }
    VERIFIED.lock().remove(path);
    let _ = std::fs::remove_file(path.with_extension("used"));
    let _ = std::fs::remove_file(manifest_path(path));
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir_all(super::text::dir_of(path));
    let mut wal = path.as_os_str().to_owned();
    wal.push(".wal");
    let _ = std::fs::remove_file(PathBuf::from(wal));
    !path.exists() && !super::text::dir_of(path).exists() && !manifest_path(path).exists()
}

/// Lines parsed and converted per parallel task.
const CHUNK: usize = 2_048;

/// Field facts of one chunk that the store records for query translation.
#[derive(Default)]
struct Facts {
    structured: HashSet<String>,
    ci_multi: HashSet<String>,
    overflow: HashSet<String>,
}

fn write(
    source: PartSource,
    path: &Path,
    derived: &[CompiledDerived],
    catalogs: Option<(&CodesConfig, &CodesConfig)>,
    progress: &(dyn Fn(usize, usize) + Sync),
    phase: &(dyn Fn(&str) + Sync),
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Outcome, String> {
    let empty = CodesConfig::default();
    let (codes, system) = catalogs.unwrap_or((&empty, &empty));
    let identity = source.part.identity.clone();
    let base = source.part.base;
    let idx = FileIndex {
        parts: vec![source.part],
        lines: Arc::new(source.lines.slice(source.range)),
        columns: Vec::new(),
        time_order: std::sync::Arc::new(std::sync::OnceLock::new()),
    };
    let total = idx.lines.len();
    if total > u32::MAX as usize {
        return Err("Arquivo grande demais para o índice de consultas.".into());
    }
    phase("Escolhendo colunas");
    if cancelled() {
        return Err("Operação cancelada.".into());
    }
    let wide_fields = choose_wide(&idx, codes, system, derived);
    let wide_names: Vec<String> = (0..wide_fields.len()).map(|i| format!("f{i}")).collect();
    let wide_index: HashMap<String, usize> = wide_fields
        .iter()
        .enumerate()
        .map(|(i, k)| (k.clone(), i))
        .collect();
    let schema = table_schema(wide_fields.len());
    // Inverted index of the free text, built alongside (see `text`).
    let text_threads = crate::resources::text_threads().min(total.div_ceil(32_768).max(1));
    let input_bytes = idx
        .lines
        .last()
        .zip(idx.lines.first())
        .map(|(last, first)| last.offset + u64::from(last.len) - first.offset)
        .unwrap_or(0);
    let text_memory = crate::resources::text_memory_bytes()
        .min(input_bytes.saturating_mul(2).saturating_add(32 << 20) as usize);
    let text = super::text::Writer::create(&super::text::dir_of(path), text_threads, text_memory)?;
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    super::limit_resources(&conn, true)?;
    create_table(&conn, &wide_names).map_err(|e| e.to_string())?;
    // Each writer appends one contiguous share of the file inside its own
    // transaction: full row groups go straight to the file, compressed in
    // parallel, instead of a commit, log write and checkpoint per batch.
    // Machines with little memory keep one writer and smaller batches.
    let low_memory = crate::resources::low_memory();
    let writers = if low_memory {
        1
    } else {
        (crate::resources::workers() / 4).clamp(1, 4)
    };
    let step = CHUNK * if low_memory { 4 } else { 16 };
    let mut connections = Vec::with_capacity(writers);
    for _ in 1..writers {
        connections.push(conn.try_clone().map_err(|e| e.to_string())?);
    }
    connections.push(conn);
    let share = total.div_ceil(writers).div_ceil(step) * step;
    let mut cursors: Vec<(usize, usize)> = (0..writers)
        .map(|k| ((k * share).min(total), ((k + 1) * share).min(total)))
        .collect();

    phase("Convertendo e indexando registros");
    let mut facts = Facts::default();
    let started = std::time::Instant::now();
    let conn = std::thread::scope(|scope| -> Result<Connection, String> {
        let mut senders = Vec::with_capacity(writers);
        let mut handles = Vec::with_capacity(writers);
        for conn in connections {
            let (sender, receiver) =
                std::sync::mpsc::sync_channel::<RecordBatch>(crate::resources::queue_batches());
            senders.push(sender);
            handles.push(scope.spawn(move || -> Result<Connection, String> {
                crate::resources::lower_priority();
                let mut busy = std::time::Duration::ZERO;
                conn.execute_batch("BEGIN TRANSACTION")
                    .map_err(|e| e.to_string())?;
                {
                    let mut appender = conn.appender("ev").map_err(|e| e.to_string())?;
                    for batch in receiver {
                        if cancelled() {
                            return Err("Operação cancelada.".into());
                        }
                        let t = std::time::Instant::now();
                        appender
                            .append_record_batch(batch)
                            .map_err(|e| e.to_string())?;
                        busy += t.elapsed();
                    }
                    let t = std::time::Instant::now();
                    appender.flush().map_err(|e| e.to_string())?;
                    busy += t.elapsed();
                }
                if cancelled() {
                    return Err("Operação cancelada.".into());
                }
                phase("Confirmando gravação do checkpoint");
                let t = std::time::Instant::now();
                conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
                trace(&format!(
                    "gravação: ocupada {busy:?}, commit {:?}",
                    t.elapsed()
                ));
                Ok(conn)
            }));
        }
        let mut produced: Result<(), String> = Ok(());
        let mut wait = std::time::Duration::ZERO;
        let mut done = 0usize;
        let mut writer = 0usize;
        'batches: while done < total {
            if cancelled() {
                produced = Err("Operação cancelada.".into());
                break;
            }
            // Next step of the next writer that still has lines.
            while cursors[writer].0 >= cursors[writer].1 {
                writer = (writer + 1) % writers;
            }
            let start = cursors[writer].0;
            let row_end = (start + step).min(cursors[writer].1);
            // A coordinated raw-byte target leaves enough records for several
            // Rayon tasks. Queues stay shallow; this is not a hard RSS limit
            // because normalization and a single oversized record can expand.
            let raw_budget = crate::resources::batch_bytes().max(64 << 10);
            let byte_end = idx.lines.at(start).offset.saturating_add(raw_budget as u64);
            let count = idx.lines.range(start..row_end).partition_point(|m| m.offset < byte_end);
            let end = (start + count.max(1)).min(row_end);
            let chunks: Vec<usize> = (start..end).step_by(CHUNK).collect();
            let text = &text;
            let parts: Vec<(RecordBatch, Facts, Result<(), String>)> = chunks
                .into_par_iter()
                .map(|from| {
                    let to = (from + CHUNK).min(end);
                    let rows: Vec<Row> = (from..to)
                        .take_while(|_| !cancelled())
                        .map(|i| {
                            let off = idx.lines.at(i).offset - base;
                            row_of(
                                event_at(&idx, i, codes, system, derived),
                                off,
                                &identity,
                                &wide_index,
                            )
                        })
                        .collect();
                    let mut found = Facts::default();
                    for row in &rows {
                        found.structured.extend(row.structured.iter().cloned());
                        found.ci_multi.extend(row.ci_multi.iter().cloned());
                        found
                            .overflow
                            .extend(row.over.iter().map(|(key, _)| key.clone()));
                    }
                    // Each parsing task feeds the text index directly.
                    let indexed = rows.iter().enumerate().try_for_each(|(r, row)| {
                        let mut words = super::text::words(&row.vals);
                        words.extend(super::text::words(&row.pname.to_lowercase()));
                        words.extend(super::text::words(&row.pdesc.to_lowercase()));
                        text.add((from + r) as u32, words)
                    });
                    (
                        batch_of(&rows, from as u32, wide_fields.len(), &schema),
                        found,
                        indexed,
                    )
                })
                .collect();
            if cancelled() {
                produced = Err("Operação cancelada.".into());
                break;
            }
            for (batch, found, indexed) in parts {
                if let Err(error) = indexed {
                    produced = Err(error);
                    break 'batches;
                }
                facts.structured.extend(found.structured);
                facts.ci_multi.extend(found.ci_multi);
                facts.overflow.extend(found.overflow);
                let t = std::time::Instant::now();
                // A full writer queue must not hide cancellation indefinitely.
                let mut pending_batch = batch;
                loop {
                    if cancelled() {
                        produced = Err("Operação cancelada.".into());
                        break 'batches;
                    }
                    match senders[writer].try_send(pending_batch) {
                        Ok(()) => break,
                        Err(std::sync::mpsc::TrySendError::Full(batch)) => {
                            pending_batch = batch;
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                            produced = Err("O gravador do checkpoint foi interrompido.".into());
                            break 'batches;
                        }
                    }
                }
                wait += t.elapsed();
            }
            cursors[writer].0 = end;
            done += end - start;
            writer = (writer + 1) % writers;
            progress(done, total);
        }
        drop(senders);
        trace(&format!(
            "leitura e conversão {:?} (espera da gravação {wait:?}), {writers} gravadores",
            started.elapsed()
        ));
        let mut written = Vec::with_capacity(writers);
        for handle in handles {
            written.push(
                handle
                    .join()
                    .map_err(|_| "Falha ao gravar o índice de consultas.".to_string())?,
            );
        }
        produced?;
        let mut kept = None;
        for conn in written {
            kept = Some(conn?);
        }
        kept.ok_or_else(|| "Falha ao gravar o índice de consultas.".to_string())
    })?;
    if cancelled() {
        return Err("Operação cancelada.".into());
    }
    phase("Concluindo e unindo o índice de texto");
    text.finish()?;
    if cancelled() {
        return Err("Operação cancelada.".into());
    }
    let mut overflow: Vec<String> = facts.overflow.into_iter().collect();
    overflow.sort();
    let mut structured: Vec<String> = facts.structured.into_iter().collect();
    structured.sort();
    let mut ci_multi: Vec<String> = facts.ci_multi.into_iter().collect();
    ci_multi.sort();
    let info = StoreInfo {
        version: STORE_VERSION,
        rows: total,
        wide: wide_fields.into_iter().zip(wide_names).collect(),
        overflow,
        structured,
        ci_multi,
        baked: catalogs.is_some(),
    };
    let text = serde_json::to_string(&info).map_err(|e| e.to_string())?;
    conn.execute("INSERT INTO meta VALUES ('info', ?)", [text])
        .map_err(|e| e.to_string())?;
    let t = std::time::Instant::now();
    phase("Sincronizando checkpoint no disco");
    conn.execute_batch("CHECKPOINT")
        .map_err(|e| e.to_string())?;
    drop(conn);
    trace(&format!("checkpoint final {:?}", t.elapsed()));
    Ok(Outcome { info })
}

/// Build timings on stderr when `LOGINSIGHT_ENGINE_TRACE` is set.
fn trace(message: &str) {
    if std::env::var_os("LOGINSIGHT_ENGINE_TRACE").is_some() {
        eprintln!("[motor] {message}");
    }
}

/// Reads the layout written by [`build`].
pub(crate) fn read_info(conn: &Connection, alias: &str) -> Result<StoreInfo, String> {
    let text: String = conn
        .query_row(
            &format!("SELECT v FROM {alias}.meta WHERE k = 'info'"),
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let info: StoreInfo = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if info.version != STORE_VERSION {
        return Err("Versão de índice de consultas antiga.".into());
    }
    Ok(info)
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn fixture() -> (tempfile::TempDir, FileIndex) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let body = (0..24).map(|i| format!("{{\"timestamp\":1700000000000,\"message\":\"alpha record {i}\",\"trace_id\":\"trace-{i}\"}}\n")).collect::<String>();
        std::fs::write(&path, body).unwrap();
        let idx =
            crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        (dir, idx)
    }
    fn source(idx: &FileIndex, range: std::ops::Range<usize>) -> PartSource {
        copy_part(&idx.parts[0], Arc::clone(&idx.lines), range)
    }
    fn valid(idx: &FileIndex, target: &Path, range: std::ops::Range<usize>) -> bool {
        published_matches(
            target,
            &idx.parts[0].identity,
            range.len(),
            idx.lines.at(range.start).offset,
            idx.lines.at(range.end - 1).offset + u64::from(idx.lines.at(range.end - 1).len),
        )
    }
    #[test]
    fn complete_checkpoint_is_reused_without_reparsing() {
        let (dir, idx) = fixture();
        let target = dir.path().join("first.duckdb");
        build(
            source(&idx, 0..12),
            &target,
            &[],
            None,
            &|_, _| {},
            &|_| {},
            &|| false,
        )
        .unwrap();
        assert!(valid(&idx, &target, 0..12));
        let calls = AtomicUsize::new(0);
        build(
            source(&idx, 0..12),
            &target,
            &[],
            None,
            &|_, _| {
                calls.fetch_add(1, Ordering::Relaxed);
            },
            &|_| {},
            &|| false,
        )
        .unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert!(
            !valid(&idx, &target, 1..13),
            "record bounds are part of the checkpoint"
        );
    }
    #[test]
    fn interrupted_later_segment_preserves_completed_checkpoint() {
        let (dir, idx) = fixture();
        let first = dir.path().join("first.duckdb");
        let second = dir.path().join("second.duckdb");
        build(
            source(&idx, 0..12),
            &first,
            &[],
            None,
            &|_, _| {},
            &|_| {},
            &|| false,
        )
        .unwrap();
        let cancelled = AtomicBool::new(false);
        let result = build(
            source(&idx, 12..24),
            &second,
            &[],
            None,
            &|_, _| {
                cancelled.store(true, Ordering::Relaxed);
            },
            &|_| {},
            &|| cancelled.load(Ordering::Relaxed),
        );
        assert!(result.is_err());
        assert!(valid(&idx, &first, 0..12));
        assert!(!valid(&idx, &second, 12..24));
        build(
            source(&idx, 12..24),
            &second,
            &[],
            None,
            &|_, _| {},
            &|_| {},
            &|| false,
        )
        .unwrap();
        assert!(valid(&idx, &second, 12..24));
    }
    #[test]
    fn missing_marker_and_text_segment_are_not_ready_and_can_recover() {
        let (dir, idx) = fixture();
        let target = dir.path().join("recover.duckdb");
        build(
            source(&idx, 0..24),
            &target,
            &[],
            None,
            &|_, _| {},
            &|_| {},
            &|| false,
        )
        .unwrap();
        std::fs::remove_file(manifest_path(&target)).unwrap();
        assert!(!valid(&idx, &target, 0..24));
        build(
            source(&idx, 0..24),
            &target,
            &[],
            None,
            &|_, _| {},
            &|_| {},
            &|| false,
        )
        .unwrap();
        let manifest: Manifest =
            serde_json::from_slice(&std::fs::read(manifest_path(&target)).unwrap()).unwrap();
        let segment = manifest
            .artifacts
            .iter()
            .find(|a| a.name != "database" && !a.name.starts_with('.') && a.name != "meta.json")
            .unwrap();
        std::fs::remove_file(super::super::text::dir_of(&target).join(&segment.name)).unwrap();
        assert!(!valid(&idx, &target, 0..24));
        build(
            source(&idx, 0..24),
            &target,
            &[],
            None,
            &|_, _| {},
            &|_| {},
            &|| false,
        )
        .unwrap();
        assert!(valid(&idx, &target, 0..24));
    }
    #[test]
    fn same_length_corruption_is_rejected_on_validation() {
        use std::io::{Read, Seek, SeekFrom, Write};
        let (dir, idx) = fixture();
        let target = dir.path().join("corrupt.duckdb");
        build(
            source(&idx, 0..24),
            &target,
            &[],
            None,
            &|_, _| {},
            &|_| {},
            &|| false,
        )
        .unwrap();
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&target)
            .unwrap();
        let mut byte = [0];
        file.read_exact(&mut byte).unwrap();
        byte[0] ^= 1;
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(&byte).unwrap();
        file.sync_all().unwrap();
        drop(file);
        VERIFIED.lock().remove(&target); // simulate reopening in a fresh process
        assert!(!valid(&idx, &target, 0..24));
    }
    #[test]
    fn source_change_during_build_is_not_published() {
        use std::io::Write;
        let (dir, idx) = fixture();
        let target = dir.path().join("changed.duckdb");
        let changed = AtomicBool::new(false);
        let result = build(
            source(&idx, 0..24),
            &target,
            &[],
            None,
            &|_, _| {
                if !changed.swap(true, Ordering::Relaxed) {
                    let mut file = std::fs::OpenOptions::new()
                        .append(true)
                        .open(&idx.parts[0].physical_path)
                        .unwrap();
                    file.write_all(b"\n").unwrap();
                    file.sync_all().unwrap();
                }
            },
            &|_| {},
            &|| false,
        );
        assert!(matches!(result, Err(error) if error.contains("alterada")));
        assert!(!manifest_path(&target).exists());
    }
    #[test]
    fn active_session_reader_lease_blocks_eviction_lock() {
        let (dir, idx) = fixture();
        let target = dir.path().join("lease.duckdb");
        build(
            source(&idx, 0..24),
            &target,
            &[],
            None,
            &|_, _| {},
            &|_| {},
            &|| false,
        )
        .unwrap();
        let last = idx.lines.last().unwrap();
        let spec = super::super::SourceSpec {
            key: "lease@0".into(),
            baked: false,
            parts: vec![super::super::PartSpec {
                key: "lease".into(),
                path: target.clone(),
                start: 0,
                rows: 24,
                part: 0,
                identity: idx.parts[0].identity.clone(),
                first_offset: 0,
                last_end: last.offset + u64::from(last.len),
            }],
        };
        let session = super::super::Session::open(&spec, None).unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(target.with_extension("build.lock"))
            .unwrap();
        assert!(fs2::FileExt::try_lock_exclusive(&lock).is_err());
        drop(session);
        fs2::FileExt::try_lock_exclusive(&lock).unwrap();
    }
}

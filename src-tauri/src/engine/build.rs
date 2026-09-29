//! Builds the columnar store of one file part from the very events the app
//! materializes (`event_at`), so every column holds the value the line
//! engine would compute.
use crate::entities::{self, ROLES};
use crate::model::{label_class, CodesConfig, Event, LineMeta, LV_OTHER};
use crate::sources::{event_at, CompiledDerived, FileIndex, FilePart};
use duckdb::arrow::array::{
    new_null_array, ArrayRef, Int64Array, ListBuilder, StringArray, StringBuilder, UInt32Array, UInt64Array,
    UInt8Array,
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
pub(crate) const STORE_VERSION: u32 = 1;
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

/// Copy of a part that stays valid after the app's locks are released.
pub(crate) struct PartSource {
    pub part: FilePart,
    pub lines: Vec<LineMeta>,
}

pub(crate) fn copy_part(part: &FilePart, lines: &[LineMeta]) -> PartSource {
    PartSource {
        part: FilePart {
            path: part.path.clone(),
            file_name: part.file_name.clone(),
            format: part.format.clone(),
            custom: part.custom.clone(),
            ts_config: part.ts_config.clone(),
            header: part.header.clone(),
            mmap: Arc::clone(&part.mmap),
            base: part.base,
            identity: part.identity.clone(),
        },
        lines: lines.to_vec(),
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
fn free_text(ev: &Event) -> String {
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
    let list = |name: &str| Field::new(name, DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))), true);
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
        Arc::new(UInt32Array::from_iter_values((0..n as u32).map(|r| first + r))),
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
        arrays.push(optional(rows.iter().map(|r| r.roles[slot].as_deref()).collect()));
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
fn choose_wide(idx: &FileIndex, codes: &CodesConfig, system: &CodesConfig, derived: &[CompiledDerived]) -> Vec<String> {
    let n = idx.lines.len();
    let head = n.min(SAMPLE / 2);
    let mut positions: Vec<usize> = (0..head).collect();
    let spread = (n - head).min(SAMPLE / 2);
    positions.extend((0..spread).map(|k| head + k * (n - head) / spread.max(1)));
    let seen: Vec<Vec<String>> = positions
        .par_iter()
        .map(|&i| event_at(idx, i, codes, system, derived).fields.keys().cloned().collect())
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

/// Builds the store at `target`, reporting `(done, total)` lines. The file
/// appears only when complete; failures and cancellation leave nothing behind.
pub(crate) fn build(
    source: PartSource,
    target: &Path,
    derived: &[CompiledDerived],
    catalogs: Option<(&CodesConfig, &CodesConfig)>,
    progress: &(dyn Fn(usize, usize) + Sync),
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Outcome, String> {
    let pending = target.with_extension("pending");
    remove_database(&pending);
    let result = write(source, &pending, derived, catalogs, progress, cancelled);
    match result {
        Ok(outcome) => {
            let _ = std::fs::remove_file(target);
            std::fs::rename(&pending, target).map_err(|e| {
                remove_database(&pending);
                format!("Não foi possível concluir o índice de consultas: {e}")
            })?;
            Ok(outcome)
        }
        Err(error) => {
            remove_database(&pending);
            Err(error)
        }
    }
}

pub(crate) fn remove_database(path: &Path) {
    let _ = std::fs::remove_file(path);
    let mut wal = path.as_os_str().to_owned();
    wal.push(".wal");
    let _ = std::fs::remove_file(PathBuf::from(wal));
}

/// Lines parsed and converted per parallel task.
const CHUNK: usize = 2_048;
/// Lines handed to the thread pool at a time, for one writer.
const STEP: usize = 16 * CHUNK;

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
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Outcome, String> {
    let empty = CodesConfig::default();
    let (codes, system) = catalogs.unwrap_or((&empty, &empty));
    let identity = source.part.identity.clone();
    let base = source.part.base;
    let idx = FileIndex {
        parts: vec![source.part],
        lines: source.lines,
        columns: Vec::new(),
        time_order: std::sync::OnceLock::new(),
    };
    let total = idx.lines.len();
    if total > u32::MAX as usize {
        return Err("Arquivo grande demais para o índice de consultas.".into());
    }
    let wide_fields = choose_wide(&idx, codes, system, derived);
    let wide_names: Vec<String> = (0..wide_fields.len()).map(|i| format!("f{i}")).collect();
    let wide_index: HashMap<String, usize> =
        wide_fields.iter().enumerate().map(|(i, k)| (k.clone(), i)).collect();
    let schema = table_schema(wide_fields.len());
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    create_table(&conn, &wide_names).map_err(|e| e.to_string())?;
    // Each writer appends one contiguous share of the file inside its own
    // transaction: full row groups go straight to the file, compressed in
    // parallel, instead of a commit, log write and checkpoint per batch.
    let writers = (rayon::current_num_threads() / 4).clamp(1, 4);
    let mut connections = Vec::with_capacity(writers);
    for _ in 1..writers {
        connections.push(conn.try_clone().map_err(|e| e.to_string())?);
    }
    connections.push(conn);
    let share = total.div_ceil(writers).div_ceil(STEP) * STEP;
    let mut cursors: Vec<(usize, usize)> =
        (0..writers).map(|k| ((k * share).min(total), ((k + 1) * share).min(total))).collect();

    let mut facts = Facts::default();
    let started = std::time::Instant::now();
    let conn = std::thread::scope(|scope| -> Result<Connection, String> {
        let mut senders = Vec::with_capacity(writers);
        let mut handles = Vec::with_capacity(writers);
        for conn in connections {
            let (sender, receiver) = std::sync::mpsc::sync_channel::<RecordBatch>(STEP / CHUNK);
            senders.push(sender);
            handles.push(scope.spawn(move || -> Result<Connection, String> {
                let mut busy = std::time::Duration::ZERO;
                conn.execute_batch("BEGIN TRANSACTION").map_err(|e| e.to_string())?;
                {
                    let mut appender = conn.appender("ev").map_err(|e| e.to_string())?;
                    for batch in receiver {
                        let t = std::time::Instant::now();
                        appender.append_record_batch(batch).map_err(|e| e.to_string())?;
                        busy += t.elapsed();
                    }
                    let t = std::time::Instant::now();
                    appender.flush().map_err(|e| e.to_string())?;
                    busy += t.elapsed();
                }
                let t = std::time::Instant::now();
                conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
                trace(&format!("gravação: ocupada {busy:?}, commit {:?}", t.elapsed()));
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
            let end = (start + STEP).min(cursors[writer].1);
            let chunks: Vec<usize> = (start..end).step_by(CHUNK).collect();
            let parts: Vec<(RecordBatch, Facts)> = chunks
                .into_par_iter()
                .map(|from| {
                    let to = (from + CHUNK).min(end);
                    let rows: Vec<Row> = (from..to)
                        .map(|i| {
                            let off = idx.lines[i].offset - base;
                            row_of(event_at(&idx, i, codes, system, derived), off, &identity, &wide_index)
                        })
                        .collect();
                    let mut found = Facts::default();
                    for row in &rows {
                        found.structured.extend(row.structured.iter().cloned());
                        found.ci_multi.extend(row.ci_multi.iter().cloned());
                        found.overflow.extend(row.over.iter().map(|(key, _)| key.clone()));
                    }
                    (batch_of(&rows, from as u32, wide_fields.len(), &schema), found)
                })
                .collect();
            for (batch, found) in parts {
                facts.structured.extend(found.structured);
                facts.ci_multi.extend(found.ci_multi);
                facts.overflow.extend(found.overflow);
                let t = std::time::Instant::now();
                if senders[writer].send(batch).is_err() {
                    break 'batches;
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
    conn.execute_batch("CHECKPOINT").map_err(|e| e.to_string())?;
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
        .query_row(&format!("SELECT v FROM {alias}.meta WHERE k = 'info'"), [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let info: StoreInfo = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if info.version != STORE_VERSION {
        return Err("Versão de índice de consultas antiga.".into());
    }
    Ok(info)
}

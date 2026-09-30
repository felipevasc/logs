//! Operations answered by the engine. Each returns `None` when the engine is
//! not ready or fails, and the caller then runs the line engine.
use super::sql::lit;
use super::udf::Tests;
use super::{session, Session};
use crate::analysis::{SeriesData, SeriesResult, SeriesSpec, UnitKind};
use crate::insights::{Comparison, Overview, Pattern, Period};
use crate::model::{class_label, ts_to_iso, CodesConfig, Event};
use crate::query::{
    build_agg_result, build_stats, is_meta_column, sort_levels, stats_layout, Acc, AggResult, AggSpec,
    ExplorerSnapshot, PreparedFilter, QueryResult, Stats,
};
use crate::sources::{event_at, CompiledDerived, FileIndex};
use duckdb::arrow::array::{Array, Int64Array};
use duckdb::arrow::datatypes::{DataType, Field, Schema};
use duckdb::arrow::record_batch::RecordBatch;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The loaded source and the catalogs its events are read with.
pub(crate) struct Source<'a> {
    pub idx: &'a FileIndex,
    pub codes: &'a CodesConfig,
    pub system: &'a CodesConfig,
    pub derived: &'a [CompiledDerived],
}

impl Source<'_> {
    fn event(&self, id: usize) -> Event {
        event_at(self.idx, id, self.codes, self.system, self.derived)
    }
}

type Result<T> = std::result::Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The handle belongs to this checked-out connection only. Joining the watcher
/// before returning it to the pool prevents a late cancellation from interrupting
/// a different operation that reuses the connection.
struct InterruptWatch {
    stop: std::sync::mpsc::Sender<()>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Drop for InterruptWatch {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}

fn interruptible(conn: &duckdb::Connection) -> Result<Option<InterruptWatch>> {
    crate::operations::check()?;
    if crate::operations::current_generation().is_none() && crate::operations::current_id().is_none() {
        return Ok(None);
    }
    let token = crate::operations::current_token();
    let interrupt = conn.interrupt_handle();
    let (stop, finished) = std::sync::mpsc::channel();
    let worker = std::thread::Builder::new().name("loginsight-query-cancel".into()).spawn(move || {
        loop {
            match finished.recv_timeout(std::time::Duration::from_millis(25)) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if token.cancelled() { interrupt.interrupt(); break; }
                }
            }
        }
    }).map_err(err)?;
    Ok(Some(InterruptWatch { stop, worker: Some(worker) }))
}

/// Runs `f` on the source's session; errors fall back to the line engine.
fn with<T>(src: &Source, f: impl FnOnce(&Session) -> Result<T>) -> Option<T> {
    with_ready(src, false, f)
}

fn ready_session(src: &Source, base_allowed: bool) -> Option<Arc<Session>> {
    if base_allowed && !src.derived.is_empty() {
        if let Some(base) = super::base_session(src.idx, src.codes, src.system) { return Some(base); }
    }
    session(src.idx, src.codes, src.system, src.derived)
}

fn with_ready<T>(src: &Source, base_allowed: bool, f: impl FnOnce(&Session) -> Result<T>) -> Option<T> {
    let session = ready_session(src, base_allowed)?;
    let _names = session.names_guard();
    match f(&session) {
        Ok(value) if !crate::operations::cancelled() => Some(value),
        Ok(_) => None,
        Err(error) => {
            if !crate::operations::cancelled() {
                eprintln!("[motor] consulta respondida pelo motor de linhas: {error}");
            }
            None
        }
    }
}

// ---------------------------------------------------------------- scopes

/// Rows selected by a filter list, as a condition the queries reuse.
struct Scope<'s> {
    session: &'s Session,
    cond: String,
    names: bool,
    _tests: Tests,
    _selection: Option<Arc<Selection>>,
    _free: Vec<Arc<Selection>>,
}

/// Rows checked at most per free-text needle found through the inverted index.
const FREE_LIMIT: usize = 250_000;

/// Replaces each `contains(vals, '…')` of a condition by the rows whose text
/// contains the needle: candidates come from the inverted index and are
/// confirmed with the same text the column holds. Needles the index cannot
/// narrow keep the scan.
fn resolve_free(session: &Session, src: &Source, sql: &str) -> Result<(String, Vec<Arc<Selection>>)> {
    const OPEN: &str = "contains(vals, '";
    let mut out = String::with_capacity(sql.len());
    let mut held = Vec::new();
    let mut rest = sql;
    while let Some(at) = rest.find(OPEN) {
        out.push_str(&rest[..at]);
        let body = &rest[at + OPEN.len()..];
        // SQL literal: '' is a quote inside it.
        let mut needle = String::new();
        let mut chars = body.char_indices().peekable();
        let mut end = None;
        while let Some((i, c)) = chars.next() {
            if c == '\'' {
                if chars.peek().map(|(_, n)| *n) == Some('\'') {
                    chars.next();
                    needle.push('\'');
                } else {
                    end = Some(i + 1);
                    break;
                }
            } else {
                needle.push(c);
            }
        }
        let Some(end) = end.filter(|&e| body[e..].starts_with(')')) else {
            out.push_str(&rest[at..]);
            return Ok((out, held));
        };
        let whole = &rest[at..at + OPEN.len() + end + 1];
        match free_selection(session, src, &needle)? {
            Some(selection) => {
                out.push_str(&format!("id IN (SELECT id FROM {})", selection.name));
                held.push(selection);
            }
            None => out.push_str(whole),
        }
        rest = &body[end + 1..];
    }
    out.push_str(rest);
    Ok((out, held))
}

fn free_selection(session: &Session, src: &Source, needle: &str) -> Result<Option<Arc<Selection>>> {
    let key = format!("free#{needle}");
    if let Some(found) = session.cached_selection(&key) {
        return Ok(Some(found));
    }
    let Some(candidates) = session.free_candidates(needle, FREE_LIMIT) else { return Ok(None) };
    // Confirmed on the stored text of the candidates only (the original file
    // would be read line by line, slowly on a cold disk).
    let _ = src;
    let pending = selection(session, &candidates)?;
    let found = selection_query(
        session,
        &format!(
            "SELECT id FROM ev WHERE id IN (SELECT id FROM {}) AND contains(vals, {}) ORDER BY id",
            pending.name,
            lit(needle)
        ),
    )?;
    drop(pending);
    session.cache_selection(key, Arc::clone(&found));
    Ok(Some(found))
}

/// Table holding the ids a filter list selects; dropped with its last user.
pub(crate) struct Selection {
    name: String,
    garbage: Arc<parking_lot::Mutex<Vec<String>>>,
}

impl Drop for Selection {
    fn drop(&mut self) {
        self.garbage.lock().push(std::mem::take(&mut self.name));
    }
}

impl Scope<'_> {
    fn from(&self, names: bool) -> &'static str {
        if self.names || names {
            "evn"
        } else {
            "ev"
        }
    }
}

fn read_ids(session: &Session, sql: &str) -> Result<Vec<usize>> {
    let conn = session.conn()?;
    let _cancel = interruptible(&conn)?;
    let mut stmt = conn.prepare(sql).map_err(err)?;
    let mut ids = Vec::new();
    for batch in stmt.query_arrow([]).map_err(err)? {
        crate::operations::check()?;
        let column = batch.column(0);
        let values = column
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or("Identificadores em formato inesperado.")?;
        ids.extend(values.values().iter().map(|&v| v as usize));
    }
    Ok(ids)
}

fn intersect(a: &[usize], b: &[usize]) -> Vec<usize> {
    let (mut i, mut j, mut out) = (0, 0, Vec::with_capacity(a.len().min(b.len())));
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out
}

/// Line numbers matching every filter, in file order.
fn matching_ids(session: &Session, src: &Source, pfs: &[PreparedFilter]) -> Result<Vec<usize>> {
    let plan = session.schema.plan(pfs);
    let from = if plan.names { "evn" } else { "ev" };
    let (sql, _free) = resolve_free(session, src, &plan.sql)?;
    let mut ids = read_ids(session, &format!("SELECT id FROM {from} WHERE {sql} ORDER BY id"))?;
    drop(plan.tests);
    for &i in &plan.lines {
        let other = crate::query::scan_matches(src.idx, std::slice::from_ref(&pfs[i].f), src.codes, src.system, src.derived);
        crate::operations::check()?;
        ids = intersect(&ids, &other);
    }
    if !plan.verify.is_empty() {
        let checks: Vec<&PreparedFilter> = plan.verify.iter().map(|&i| &pfs[i]).collect();
        let cancellation = crate::operations::current_token();
        let kept: Vec<Vec<usize>> = ids
            .par_chunks(4096)
            .map(|chunk| {
                let mut out = Vec::new();
                for &id in chunk {
                    if cancellation.cancelled() {
                        break;
                    }
                    let ev = src.event(id);
                    if checks.iter().all(|pf| crate::query::matches(&ev, pf)) {
                        out.push(id);
                    }
                }
                out
            })
            .collect();
        crate::operations::check()?;
        ids = kept.concat();
    }
    Ok(ids)
}

/// Conditions that read long texts on every row (free text, messages,
/// rendered times). Their selection is computed once and reused by the
/// queries of a screen.
fn costly(sql: &str) -> bool {
    ["vals", "message", "li_iso(", "event_ref", "xv[", "(name,", "(description,"]
        .iter()
        .any(|part| sql.contains(part))
}

fn scope<'s>(session: &'s Session, src: &Source, pfs: &[PreparedFilter]) -> Result<Scope<'s>> {
    let plan = session.schema.plan(pfs);
    let (sql, free) = resolve_free(session, src, &plan.sql)?;
    if plan.exact() && !costly(&sql) {
        return Ok(Scope {
            session,
            cond: sql,
            names: plan.names,
            _tests: plan.tests,
            _selection: None,
            _free: free,
        });
    }
    // Exact selections depend only on the filters (and loaded names).
    let key = plan.exact().then(|| {
        let filters: Vec<&crate::query::Filter> = pfs.iter().map(|pf| &pf.f).collect();
        format!("{}#{}", serde_json::to_string(&filters).unwrap_or_default(), session.names_version())
    });
    if let Some(found) = key.as_deref().and_then(|key| session.cached_selection(key)) {
        return Ok(Scope {
            session,
            cond: format!("id IN (SELECT id FROM {})", found.name),
            names: false,
            _tests: Tests::default(),
            _selection: Some(found),
            _free: Vec::new(),
        });
    }
    let selection = if plan.exact() {
        let from = if plan.names { "evn" } else { "ev" };
        // Keep a potentially 50M-row selection inside DuckDB, whose buffer
        // manager can spill it, instead of a Rust Vec followed by another copy.
        selection_query(session, &format!("SELECT id FROM {from} WHERE {sql}"))?
    } else {
        drop(plan);
        selection(session, &matching_ids(session, src, pfs)?)?
    };
    if let Some(key) = key {
        session.cache_selection(key, Arc::clone(&selection));
    }
    Ok(Scope {
        session,
        cond: format!("id IN (SELECT id FROM {})", selection.name),
        names: false,
        _tests: Tests::default(),
        _selection: Some(selection),
        _free: Vec::new(),
    })
}

/// A page must not first materialize every match to populate an analytics
/// cache. Non-exact predicates are verified in bounded sorted batches below.
fn page_scope<'s>(session: &'s Session, src: &Source, pfs: &[PreparedFilter]) -> Result<(Scope<'s>, bool)> {
    let plan = session.schema.plan(pfs);
    let exact = plan.exact();
    let (sql, free) = resolve_free(session, src, &plan.sql)?;
    Ok((Scope { session, cond: sql, names: plan.names, _tests: plan.tests, _selection: None, _free: free }, exact))
}

fn new_selection(session: &Session) -> Arc<Selection> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    Arc::new(Selection {
        name: format!("sel_{}", NEXT.fetch_add(1, Ordering::Relaxed)),
        garbage: session.garbage(),
    })
}

fn selection_query(session: &Session, sql: &str) -> Result<Arc<Selection>> {
    let conn = session.conn()?;
    let _cancel = interruptible(&conn)?;
    for unused in session.take_garbage() {
        let _ = conn.execute_batch(&format!("DROP TABLE IF EXISTS {unused}"));
    }
    let selection = new_selection(session);
    conn.execute_batch(&format!("CREATE TABLE {} AS {sql}", selection.name)).map_err(err)?;
    Ok(selection)
}

fn selection(session: &Session, ids: &[usize]) -> Result<Arc<Selection>> {
    let conn = session.conn()?;
    for unused in session.take_garbage() {
        let _ = conn.execute_batch(&format!("DROP TABLE IF EXISTS {unused}"));
    }
    let selection = new_selection(session);
    let name = &selection.name;
    conn.execute_batch(&format!("CREATE TABLE {name} (id BIGINT)")).map_err(err)?;
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
    let mut appender = conn.appender(&name).map_err(err)?;
    for chunk in ids.chunks(1 << 16) {
        let array = Int64Array::from_iter_values(chunk.iter().map(|&i| i as i64));
        let batch = RecordBatch::try_new(Arc::clone(&schema), vec![Arc::new(array)]).map_err(err)?;
        appender.append_record_batch(batch).map_err(err)?;
    }
    appender.flush().map_err(err)?;
    Ok(selection)
}

fn prepared(filters: &[crate::query::Filter]) -> Vec<PreparedFilter> {
    crate::query::prepare(filters)
}

fn rows<T>(session: &Session, sql: &str, map: impl FnMut(&duckdb::Row<'_>) -> duckdb::Result<T>) -> Result<Vec<T>> {
    let conn = session.conn()?;
    let _cancel = interruptible(&conn)?;
    let mut stmt = conn.prepare(sql).map_err(err)?;
    let mapped = stmt.query_map([], map).map_err(err)?;
    mapped.collect::<duckdb::Result<Vec<T>>>().map_err(err)
}

// ---------------------------------------------------------------- matches

/// `query::indexed_matches` for prepared filters.
pub(crate) fn matches(src: &Source, pfs: &[PreparedFilter]) -> Option<Vec<usize>> {
    with(src, |session| matching_ids(session, src, pfs))
}

pub(crate) fn count(src: &Source, filters: &[crate::query::Filter]) -> Option<usize> {
    let pfs = prepared(filters);
    with_ready(src, base_page_safe(&pfs, ""), |session| {
        let scope = scope(session, src, &pfs)?;
        let counted = rows(
            session,
            &format!("SELECT count(*) FROM {} WHERE {}", scope.from(false), scope.cond),
            |r| r.get::<_, i64>(0),
        )?;
        Ok(counted.first().copied().unwrap_or(0) as usize)
    })
}

// ---------------------------------------------------------------- stats

const LEVEL_LABEL: &str = "CASE lvl WHEN 1 THEN 'Crítico' WHEN 2 THEN 'Erro' WHEN 3 THEN 'Aviso' \
     WHEN 4 THEN 'Depuração' WHEN 5 THEN 'Rastreio' ELSE 'Informação' END";

fn stats_of(scope: &Scope) -> Result<Stats> {
    let from = scope.from(false);
    let cond = &scope.cond;
    let bounds = rows(
        scope.session,
        &format!("SELECT min(NULLIF(ts, 0)), max(NULLIF(ts, 0)) FROM {from} WHERE {cond}"),
        |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?)),
    )?;
    let mut buckets = Vec::new();
    let mut bucket_ms = 0;
    if let Some((Some(min_ts), Some(max_ts))) = bounds.first().copied() {
        let count;
        (bucket_ms, count) = stats_layout(min_ts, max_ts);
        let mut counts = vec![0i64; count];
        for (b, c) in rows(
            scope.session,
            &format!(
                "SELECT (t - ({min_ts})) // {bucket_ms} AS b, count(*) FROM \
                 (SELECT NULLIF(ts, 0) AS t FROM {from} WHERE {cond}) WHERE t IS NOT NULL GROUP BY b"
            ),
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )? {
            counts[(b.max(0) as usize).min(count - 1)] += c;
        }
        buckets = counts
            .into_iter()
            .enumerate()
            .map(|(b, c)| (min_ts.saturating_add((b as i64).saturating_mul(bucket_ms)), c))
            .collect();
    }
    let mut merged: HashMap<&'static str, i64> = HashMap::new();
    for (lvl, c) in rows(
        scope.session,
        &format!("SELECT lvl, count(*) FROM {from} WHERE {cond} GROUP BY lvl"),
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
    )? {
        *merged.entry(class_label(lvl as u8)).or_default() += c;
    }
    let mut levels: Vec<(String, i64)> = merged.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    sort_levels(&mut levels);
    Ok(build_stats(buckets, bucket_ms, levels))
}

pub(crate) fn stats(src: &Source, pfs: &[PreparedFilter]) -> Option<Stats> {
    with_ready(src, base_page_safe(pfs, ""), |session| stats_of(&scope(session, src, pfs)?))
}

// ---------------------------------------------------------------- groups

/// Text of a column as `Event::col_str` gives it; in the metadata mode
/// (level and timestamp only) level is its class label and time its index time.
fn text_of(scope: &Scope, column: &str, meta: bool, names: &mut bool) -> Result<String> {
    Ok(match column {
        "timestamp" if meta => "li_iso(NULLIF(ts, 0))".into(),
        "level" if meta => LEVEL_LABEL.into(),
        "*" if meta => "CAST(NULL AS VARCHAR)".into(),
        other => scope
            .session
            .schema
            .column(other, names)
            .ok_or("Coluna disponível apenas no texto bruto.")?,
    })
}

/// Grouping key: text with empty values as NULL; time keys stay numeric.
fn key_of(scope: &Scope, column: &str, meta: bool, names: &mut bool) -> Result<(String, bool)> {
    if column == "timestamp" {
        return Ok((if meta { "NULLIF(ts, 0)" } else { "ts" }.into(), true));
    }
    let text = text_of(scope, column, meta, names)?;
    Ok((format!("CASE WHEN li_blank({text}) THEN NULL ELSE {text} END"), false))
}

fn key_text(row: &duckdb::Row<'_>, index: usize, time: bool) -> duckdb::Result<std::result::Result<Option<String>, String>> {
    if time {
        Ok(match row.get::<_, Option<i64>>(index)? {
            Some(ts) => {
                let text = ts_to_iso(ts);
                if text.trim().is_empty() {
                    Err("Data fora do intervalo representável.".into())
                } else {
                    Ok(Some(text))
                }
            }
            None => Ok(None),
        })
    } else {
        Ok(Ok(row.get::<_, Option<String>>(index)?))
    }
}

enum SpecSql {
    Count,
    Distinct,
    Numeric,
    Text,
}

fn aggregate_of(scope: &Scope, group: &str, specs: &[AggSpec]) -> Result<AggResult> {
    let meta = is_meta_column(group) && specs.iter().all(|s| is_meta_column(&s.column));
    let mut names = false;
    let (key, time) = key_of(scope, group, meta, &mut names)?;
    if !time && !specs.is_empty() && specs.iter().all(|spec| spec.func == "count") {
        return count_groups(scope, group, specs, &key, names);
    }
    let mut inner = vec!["id".to_string(), format!("{key} AS k")];
    let mut outer = vec!["k".to_string(), "count(*)".to_string()];
    let mut kinds = Vec::with_capacity(specs.len());
    let mut texts: Vec<(usize, String)> = Vec::new();
    for (i, spec) in specs.iter().enumerate() {
        match spec.func.as_str() {
            "count" => kinds.push(SpecSql::Count),
            "count_distinct" => {
                let value = match spec.column.as_str() {
                    "timestamp" => if meta { "NULLIF(ts, 0)" } else { "ts" }.to_string(),
                    "id" => "id".to_string(),
                    other => text_of(scope, other, meta, &mut names)?,
                };
                inner.push(format!("{value} AS d{i}"));
                outer.push(format!("count(DISTINCT d{i})"));
                kinds.push(SpecSql::Distinct);
            }
            "sum" | "avg" | "min" | "max" => {
                let (number, unit) = match spec.column.as_str() {
                    "timestamp" | "id" => {
                        let raw = match spec.column.as_str() {
                            "id" => "id",
                            _ if meta => "NULLIF(ts, 0)",
                            _ => "ts",
                        };
                        (
                            format!("CAST({raw} AS DOUBLE)"),
                            format!("CASE WHEN {raw} IS NULL THEN NULL ELSE 0 END"),
                        )
                    }
                    other => {
                        let text = text_of(scope, other, meta, &mut names)?;
                        (format!("li_num({text})"), format!("li_unit({text})"))
                    }
                };
                inner.push(format!("{number} AS n{i}"));
                inner.push(format!("{unit} AS u{i}"));
                let ordered = matches!(spec.func.as_str(), "sum" | "avg");
                // Sums follow file order, as the line engine adds them.
                outer.push(if ordered { format!("sum(n{i} ORDER BY id)") } else { "NULL".into() });
                outer.push(format!("count(n{i})"));
                outer.push(format!("min(n{i})"));
                outer.push(format!("max(n{i})"));
                for unit in 0..5 {
                    outer.push(format!("count(*) FILTER (WHERE u{i} = {unit})"));
                }
                kinds.push(SpecSql::Numeric);
            }
            _ => {
                texts.push((i, text_of(scope, &spec.column, meta, &mut names)?));
                kinds.push(SpecSql::Text);
            }
        }
    }
    let from = scope.from(names);
    let sql = format!(
        "SELECT {} FROM (SELECT {} FROM {from} WHERE {}) GROUP BY k LIMIT {}",
        outer.join(", "),
        inner.join(", "),
        scope.cond,
        crate::query::MAX_GROUPS + 1,
    );
    let mut failure = None;
    let grouped = rows(scope.session, &sql, |r| {
        let key = key_text(r, 0, time)?;
        let n = r.get::<_, i64>(1)? as u64;
        let mut column = 2;
        let mut accs = Vec::with_capacity(kinds.len());
        for (kind, spec) in kinds.iter().zip(specs) {
            accs.push(match kind {
                SpecSql::Count => Acc::Count(n),
                SpecSql::Distinct => {
                    column += 1;
                    Acc::Distinct(r.get::<_, i64>(column - 1)? as u64)
                }
                SpecSql::Numeric => {
                    let sum: Option<f64> = r.get(column)?;
                    let valid = r.get::<_, i64>(column + 1)? as u64;
                    let min: Option<f64> = r.get(column + 2)?;
                    let max: Option<f64> = r.get(column + 3)?;
                    let mut units = [0usize; 5];
                    for (u, slot) in units.iter_mut().enumerate() {
                        *slot = r.get::<_, i64>(column + 4 + u)? as usize;
                    }
                    column += 9;
                    match spec.func.as_str() {
                        "sum" => Acc::Sum(sum.unwrap_or(0.0), units),
                        "avg" => Acc::Avg(sum.unwrap_or(0.0), valid, units),
                        "min" => Acc::Min(min, units),
                        _ => Acc::Max(max, units),
                    }
                }
                SpecSql::Text => Acc::StrAgg(Vec::new()),
            });
        }
        Ok((key, accs))
    })?;
    if grouped.len() > crate::query::MAX_GROUPS {
        return Ok(AggResult::budget_error());
    }
    let mut groups: HashMap<Option<String>, Vec<Acc>> = HashMap::with_capacity(grouped.len());
    let mut order = Vec::with_capacity(grouped.len());
    for (key, accs) in grouped {
        match key {
            Ok(key) => {
                order.push(key.clone());
                groups.insert(key, accs);
            }
            Err(e) => failure = Some(e),
        }
    }
    if let Some(e) = failure {
        return Err(e);
    }
    for (i, text) in texts {
        let sql = format!(
            "SELECT k, v FROM (SELECT k, v, row_number() OVER (PARTITION BY k ORDER BY id) AS rn \
             FROM (SELECT id, {key} AS k, {text} AS v FROM {from} WHERE {}) WHERE v <> '') \
             WHERE rn <= 100 ORDER BY k, rn",
            scope.cond
        );
        let items = rows(scope.session, &sql, |r| Ok((key_text(r, 0, time)?, r.get::<_, String>(1)?)))?;
        for (key, value) in items {
            let key = key?;
            if let Some(Acc::StrAgg(list)) = groups.get_mut(&key).map(|accs| &mut accs[i]) {
                list.push(value);
            }
        }
    }
    Ok(build_agg_result(groups, order, group, specs))
}

/// Facets are count-only. Rank/cap their groups before crossing the database
/// boundary; the exact omitted group/record counts stay available to the UI.
fn count_groups(scope: &Scope, group: &str, specs: &[AggSpec], key: &str, names: bool) -> Result<AggResult> {
    let from = scope.from(names);
    let limit = crate::query::MAX_GROUPS;
    let sql = format!(
        "WITH grouped AS (SELECT {key} AS k, count(*)::BIGINT AS n FROM {from} WHERE {} GROUP BY k) \
         SELECT k, n, count(*) OVER (), CAST(sum(n) OVER () AS BIGINT) FROM grouped \
         ORDER BY n DESC, (li_gkey(COALESCE(k, '')) IS NULL), li_gkey(COALESCE(k, '')), \
         li_lower(COALESCE(k, '')), k NULLS FIRST LIMIT {limit}",
        scope.cond,
    );
    let grouped = rows(scope.session, &sql, |row| Ok((
        row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)? as u64,
        row.get::<_, i64>(2)? as usize, row.get::<_, i64>(3)? as u64,
    )))?;
    let (mut all_groups, mut all_records, mut retained) = (0, 0u64, 0u64);
    let mut groups = HashMap::with_capacity(grouped.len());
    let mut order = Vec::with_capacity(grouped.len());
    for (key, count, total_groups, total_records) in grouped {
        all_groups = total_groups;
        all_records = total_records;
        retained += count;
        order.push(key.clone());
        groups.insert(key, specs.iter().map(|_| Acc::Count(count)).collect());
    }
    let mut result = build_agg_result(groups, order, group, specs);
    result.omitted_groups = all_groups.saturating_sub(result.rows.len());
    result.omitted_records = all_records.saturating_sub(retained);
    Ok(result)
}

pub(crate) fn aggregate(src: &Source, pfs: &[PreparedFilter], group: &str, specs: &[AggSpec]) -> Option<AggResult> {
    let base_allowed = base_page_safe(pfs, group) && specs.iter().all(|spec| spec.func == "count" || base_field(&spec.column));
    with_ready(src, base_allowed, |session| aggregate_of(&scope(session, src, pfs)?, group, specs))
}

fn count_spec() -> [AggSpec; 1] {
    [AggSpec {
        func: "count".into(),
        column: "*".into(),
        alias: "n".into(),
    }]
}

/// Counts per value of each column, each without its own column's filters.
pub(crate) fn multi_count(
    src: &Source,
    filters: &[crate::query::Filter],
    columns: &[String],
) -> Option<Vec<(String, AggResult)>> {
    let base_allowed = base_page_safe(&prepared(filters), "") && columns.iter().all(|column| base_field(column));
    with_ready(src, base_allowed, |session| {
        let mut out = Vec::with_capacity(columns.len());
        for column in columns {
            crate::operations::check()?;
            let own: Vec<crate::query::Filter> = filters.iter().filter(|f| f.column != *column).cloned().collect();
            let pfs = prepared(&own);
            let result = aggregate_of(&scope(session, src, &pfs)?, column, &count_spec())?;
            out.push((column.clone(), result));
        }
        Ok(out)
    })
}

// ---------------------------------------------------------------- pages

#[derive(Clone, Debug, Deserialize, Serialize)]
enum CursorKey {
    Integer(i64),
    Text(String),
}

impl CursorKey {
    fn sql(&self) -> String {
        match self {
            Self::Integer(value) => value.to_string(),
            Self::Text(value) => lit(value),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct PageCursor {
    version: u8,
    fingerprint: String,
    position: usize,
    keys: Vec<CursorKey>,
}

struct SortKey {
    sql: String,
    desc: bool,
    text: bool,
}

fn page_sort(scope: &Scope, column: &str, dir: &str) -> Result<(Vec<SortKey>, bool)> {
    let desc = dir == "desc";
    let int = |sql: String, desc| SortKey { sql, desc, text: false };
    let mut names = false;
    let mut keys = match column {
        "" => vec![int("id".into(), false)],
        "id" => vec![int("id".into(), desc)],
        "timestamp" => vec![int("COALESCE(ts, 0)".into(), desc)],
        "level" => vec![int("CAST(lvl AS BIGINT)".into(), desc)],
        column => {
            let value = scope.session.schema.column(column, &mut names).ok_or("Ordenação pelo texto bruto.")?;
            vec![
                int(format!("CAST(li_nkey({value}) IS NULL AS BIGINT)"), desc),
                int(format!("COALESCE(li_nkey({value}), 0)"), desc),
                SortKey { sql: format!("li_lower(COALESCE({value}, ''))"), desc, text: true },
            ]
        }
    };
    if !matches!(column, "" | "id") {
        keys.push(int("id".into(), false));
    }
    Ok((keys, names))
}

/// Lexicographic seek using non-null sort keys and the same final id tie-break
/// as the exact page API. Sort directions can differ (descending time, id asc).
fn after_cursor(sort: &[SortKey], keys: &[CursorKey]) -> Result<String> {
    if keys.len() != sort.len() || keys.iter().zip(sort).any(|(key, sort)| matches!(key, CursorKey::Text(_)) != sort.text) {
        return Err("PAGINATION_RESET_REQUIRED: Cursor de paginação inválido. Recarregue os registros.".into());
    }
    let mut prefix = Vec::new();
    let mut alternatives = Vec::new();
    for (sort, key) in sort.iter().zip(keys) {
        let bound = key.sql();
        let mut terms = prefix.clone();
        terms.push(format!("{} {} {bound}", sort.sql, if sort.desc { "<" } else { ">" }));
        alternatives.push(format!("({})", terms.join(" AND ")));
        prefix.push(format!("{} = {bound}", sort.sql));
    }
    Ok(format!("({})", alternatives.join(" OR ")))
}

#[cfg(test)]
mod interactive_contract_tests {
    use super::*;

    fn filters(column: &str, op: &str, value: &str) -> Vec<PreparedFilter> {
        crate::query::prepare(&[crate::query::Filter { column: column.into(), op: op.into(), value: value.into(), value2: None }])
    }

    #[test]
    fn base_capability_proof_excludes_derived_and_unscoped_semantics() {
        assert!(base_page_safe(&filters("timestamp", "gte", "1000"), "timestamp"));
        assert!(base_page_safe(&filters("_all", "query", "source:api AND code:404"), "timestamp"));
        for (column, op, value) in [
            ("_all", "query", "timeout"), ("_all", "query", "source:api OR custom:x"),
            ("custom", "equals_exact", "x"), ("@user", "equals", "root"),
            ("name", "contains", "logon"), ("_all", "contains", "error"),
            ("source", "detection", "anything"),
        ] { assert!(!base_page_safe(&filters(column, op, value), "timestamp"), "{column} {op} {value}"); }
        assert!(!base_page_safe(&[], "custom"));
    }

    #[test]
    fn cursor_keys_are_typed_and_sql_literals_are_escaped() {
        let sort = vec![SortKey { sql: "value".into(), desc: true, text: true }, SortKey { sql: "id".into(), desc: false, text: false }];
        let clause = after_cursor(&sort, &[CursorKey::Text("O'Reilly".into()), CursorKey::Integer(7)]).unwrap();
        assert!(clause.contains("value < 'O''Reilly'"));
        assert!(clause.contains("value = 'O''Reilly' AND id > 7"));
        assert!(after_cursor(&sort, &[CursorKey::Integer(7), CursorKey::Integer(7)]).is_err());
        assert!(after_cursor(&sort, &[]).is_err());
    }

    #[test]
    fn named_cancel_interrupts_sql_and_cannot_poison_reused_connection() {
        let connection = duckdb::Connection::open_in_memory().unwrap();
        let token = crate::operations::token(Some("duckdb-cancel-query-test".into())).unwrap();
        let (ready, started) = std::sync::mpsc::channel();
        let cancel = std::thread::spawn(move || {
            started.recv().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
            assert!(crate::operations::cancel_id("duckdb-cancel-query-test"));
        });
        let result = crate::operations::run_with_token(token, || {
            let _watch = interruptible(&connection).unwrap();
            ready.send(()).unwrap();
            connection.query_row("SELECT sum(sqrt(i::DOUBLE)) FROM range(1000000000000) t(i)", [], |row| row.get::<_, f64>(0))
        });
        cancel.join().unwrap();
        assert!(result.is_err());
        assert_eq!(connection.query_row("SELECT 7", [], |row| row.get::<_, i64>(0)).unwrap(), 7);
    }
}

fn base_field(name: &str) -> bool {
    matches!(name, "id" | "event_ref" | "timestamp" | "level" | "source" | "code" | "message")
}

fn base_expression(expr: &crate::querylang::Expr) -> bool {
    use crate::querylang::{Expr, TermKind};
    match expr {
        Expr::All => true,
        Expr::And(items) | Expr::Or(items) => items.iter().all(base_expression),
        Expr::Not(inner) => base_expression(inner),
        Expr::Term(term) => !matches!(term.kind(), TermKind::Event)
            && term.field().is_some_and(|(name, role, _)| role.is_none() && base_field(name)),
    }
}

/// Derived fields are hydrated from the current definitions after selection.
/// Only predicates/sorts proven independent of them may use the base store.
pub(crate) fn base_page_safe(pfs: &[PreparedFilter], sort: &str) -> bool {
    (sort.is_empty() || base_field(sort)) && pfs.iter().all(|pf| {
        match pf.f.op.as_str() {
            "query" => pf.expr.as_ref().is_some_and(base_expression),
            "threat_rule" | "detection" => false,
            _ => base_field(&pf.f.column),
        }
    })
}

/// Page-size work in Rust and no mandatory COUNT. Returns None only when no
/// suitable store is ready; the caller can explicitly report the line fallback.
pub(crate) fn query_page(
    src: &Source,
    pfs: &[PreparedFilter],
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
    cursor: Option<&str>,
) -> Option<Result<crate::query::QueryPage>> {
    for part in &src.idx.parts {
        if let Err(error) = crate::sources::validate_source(part) { return Some(Err(error)); }
    }
    if sort_column == "raw" { return None; }
    let limit = limit.clamp(1, 2_000);
    let base_safe = base_page_safe(pfs, sort_column);
    let session = ready_session(src, base_safe)?;
    let _names = session.names_guard();
    Some((|| {
        crate::operations::check()?;
        let source_key = super::spec(src.idx, src.codes, src.system, if base_safe { &[] } else { src.derived })
            .ok_or("Fonte indisponível para paginação.")?.key;
        let filters: Vec<_> = pfs.iter().map(|pf| &pf.f).collect();
        let mut hash = Sha256::new();
        hash.update(source_key);
        hash.update(super::catalogs_signature(src.codes, src.system));
        hash.update(serde_json::to_vec(&(filters, sort_column, sort_dir)).map_err(err)?);
        let fingerprint = format!("{:x}", hash.finalize());
        let (scope, exact) = page_scope(&session, src, pfs)?;
        let (sort, names) = page_sort(&scope, sort_column, sort_dir)?;
        let (after, position) = match cursor {
            Some(value) => {
                if value.len() > 1_100_000 {
                    return Err("PAGINATION_RESET_REQUIRED: Cursor de paginação excede o limite.".into());
                }
                let cursor: PageCursor = serde_json::from_str(value).map_err(|_| "PAGINATION_RESET_REQUIRED: Cursor de paginação inválido.".to_string())?;
                if cursor.version != 1 || cursor.fingerprint != fingerprint {
                    return Err("PAGINATION_RESET_REQUIRED: A consulta mudou. Recarregue a primeira página.".into());
                }
                if cursor.position > src.idx.lines.len() {
                    return Err("PAGINATION_RESET_REQUIRED: Posição de paginação inválida.".into());
                }
                (after_cursor(&sort, &cursor.keys)?, cursor.position)
            }
            None => ("TRUE".into(), offset),
        };
        let order = sort.iter().map(|key| format!("{} {}", key.sql, if key.desc { "DESC" } else { "ASC" })).collect::<Vec<_>>().join(", ");
        let projection = sort.iter().map(|key| key.sql.as_str()).collect::<Vec<_>>().join(", ");
        let mut offset_sql = if cursor.is_some() || !exact { 0 } else { offset };
        let mut skip_matches = if cursor.is_none() && !exact { offset } else { 0 };
        let mut seek = after;
        let mut found = Vec::new();
        let batch_size = if exact { limit.saturating_add(1) } else { limit.saturating_add(1).max(1_024) };
        loop {
            crate::operations::check()?;
            let sql = format!("SELECT id, {projection} FROM {} WHERE ({}) AND {seek} ORDER BY {order} LIMIT {batch_size} OFFSET {offset_sql}", scope.from(names), scope.cond);
            let candidates = rows(&session, &sql, |row| {
                let id = row.get::<_, i64>(0)? as usize;
                let keys = sort.iter().enumerate().map(|(i, key)| {
                    if key.text { row.get::<_, String>(i + 1).map(CursorKey::Text) }
                    else { row.get::<_, i64>(i + 1).map(CursorKey::Integer) }
                }).collect::<duckdb::Result<Vec<_>>>()?;
                Ok((id, keys))
            })?;
            let exhausted = candidates.len() < batch_size;
            let last_keys = candidates.last().map(|(_, keys)| keys.clone());
            for candidate in candidates {
                crate::operations::check()?;
                if !exact {
                    let event = src.event(candidate.0);
                    if !pfs.iter().all(|pf| crate::query::matches(&event, pf)) { continue; }
                }
                if skip_matches > 0 { skip_matches -= 1; continue; }
                found.push(candidate);
                if found.len() > limit { break; }
            }
            if found.len() > limit || exhausted { break; }
            let Some(keys) = last_keys else { break };
            seek = after_cursor(&sort, &keys)?;
            offset_sql = 0;
        }
        crate::operations::check()?;
        let has_more = found.len() > limit;
        found.truncate(limit);
        let next_cursor = if has_more {
            found.last().map(|(_, keys)| serde_json::to_string(&PageCursor { version: 1, fingerprint, position: position.saturating_add(found.len()), keys: keys.clone() }).map_err(err)).transpose()?
        } else { None };
        let total = if pfs.is_empty() { Some(src.idx.lines.len()) }
            else if !has_more && (position == 0 || !found.is_empty()) { Some(position.saturating_add(found.len())) }
            else { None };
        let rows = page_rows(src, found.into_iter().map(|(id, _)| id).collect());
        crate::operations::check()?;
        Ok(crate::query::QueryPage {
            rows, total, has_more, next_cursor, engine: "columnar".into(),
            warning: (!exact).then(|| "Este filtro exige confirmação nos registros; consultas amplas podem demorar mais.".into()),
        })
    })())
}

/// Reproducible test/benchmark diagnostics of the actual interactive SQL path.
/// This is intentionally not a desktop command or an automatic full-data probe.
pub(crate) fn explain_page(
    src: &Source, pfs: &[PreparedFilter], sort_column: &str, sort_dir: &str,
    limit: usize, analyze: bool,
) -> Option<Result<Value>> {
    let session = ready_session(src, base_page_safe(pfs, sort_column))?;
    let _names = session.names_guard();
    Some((|| {
        let (scope, exact) = page_scope(&session, src, pfs)?;
        let (sort, names) = page_sort(&scope, sort_column, sort_dir)?;
        let order = sort.iter().map(|key| format!("{} {}", key.sql, if key.desc { "DESC" } else { "ASC" })).collect::<Vec<_>>().join(", ");
        let sql = format!("SELECT id FROM {} WHERE {} ORDER BY {order} LIMIT {}", scope.from(names), scope.cond, limit.clamp(1, 2_000));
        let plan = rows(&session, &format!("EXPLAIN {}{sql}", if analyze { "ANALYZE " } else { "" }), |row| row.get::<_, String>(1))?.join("\n");
        Ok(serde_json::json!({ "sql": sql, "plan": plan, "analyzed": analyze, "exactPredicate": exact }))
    })())
}

fn page_ids(scope: &Scope, sort_column: &str, sort_dir: &str, offset: usize, limit: usize) -> Result<(usize, Vec<usize>)> {
    let dir = if sort_dir == "desc" { "DESC" } else { "ASC" };
    let mut names = false;
    let order = match sort_column {
        "" => "id".to_string(),
        "timestamp" => format!("COALESCE(ts, 0) {dir}, id"),
        "level" => format!("lvl {dir}, id"),
        "id" => format!("id {dir}"),
        column => {
            let value = scope
                .session
                .schema
                .column(column, &mut names)
                .ok_or("Ordenação pelo texto bruto.")?;
            format!(
                "(li_nkey({value}) IS NULL) {dir}, li_nkey({value}) {dir}, \
                 li_lower(COALESCE({value}, '')) {dir}, id"
            )
        }
    };
    let from = scope.from(names);
    let total = rows(
        scope.session,
        &format!("SELECT count(*) FROM {from} WHERE {}", scope.cond),
        |r| r.get::<_, i64>(0),
    )?
    .first()
    .copied()
    .unwrap_or(0) as usize;
    let ids = if limit == 0 || offset >= total {
        Vec::new()
    } else {
        read_ids(
            scope.session,
            &format!(
                "SELECT id FROM {from} WHERE {} ORDER BY {order} LIMIT {limit} OFFSET {offset}",
                scope.cond
            ),
        )?
    };
    Ok((total, ids))
}

/// Rows of a page, read in parallel (pages of trails reach thousands).
fn page_rows(src: &Source, ids: Vec<usize>) -> Vec<Event> {
    ids.into_par_iter()
        .map(|i| {
            let mut event = src.event(i);
            crate::entities::annotate(&mut event);
            event.raw.clear();
            event
        })
        .collect()
}

pub(crate) fn query(
    src: &Source,
    pfs: &[PreparedFilter],
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
) -> Option<QueryResult> {
    with(src, |session| {
        let scope = scope(session, src, pfs)?;
        let (total, ids) = page_ids(&scope, sort_column, sort_dir, offset, limit)?;
        Ok(QueryResult {
            total,
            rows: page_rows(src, ids),
        })
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn explore(
    src: &Source,
    pfs: &[PreparedFilter],
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
) -> Option<ExplorerSnapshot> {
    with(src, |session| {
        let scope = scope(session, src, pfs)?;
        let stats = stats_of(&scope)?;
        let count = count_spec();
        let sources = aggregate_of(&scope, "source", &count)?;
        let codes = aggregate_of(&scope, "code", &count)?;
        let (total, ids) = page_ids(&scope, sort_column, sort_dir, offset, limit)?;
        Ok(ExplorerSnapshot {
            query: QueryResult {
                total,
                rows: page_rows(src, ids),
            },
            stats,
            sources,
            codes,
        })
    })
}

// ---------------------------------------------------------------- charts

/// `analysis::compute_series_stream` over the engine's columns.
pub(crate) fn series(src: &Source, pfs: &[PreparedFilter], spec: &SeriesSpec) -> Option<SeriesResult> {
    with(src, |session| series_of(&scope(session, src, pfs)?, spec))
}

/// Metric columns shared by terms and time charts: count, distinct values,
/// numeric sum (file order), valid count, min, max and incompatible units.
fn metric_columns(value: Option<&str>, expected: Option<UnitKind>) -> (String, String) {
    let Some(value) = value else {
        return (
            "NULL AS v, NULL::DOUBLE AS num, NULL::INTEGER AS u".into(),
            "count(*), 0, 0, NULL::DOUBLE, 0, NULL::DOUBLE, NULL::DOUBLE, 0".into(),
        );
    };
    let ok = match expected {
        Some(unit) => format!("num IS NOT NULL AND u = {}", unit as i32),
        None => "num IS NOT NULL".into(),
    };
    let bad = match expected {
        Some(unit) => format!("num IS NOT NULL AND u <> {}", unit as i32),
        None => "FALSE".into(),
    };
    (
        format!("{value} AS v, li_num({value}) AS num, li_unit({value}) AS u"),
        format!(
            "count(*), count(DISTINCT CASE WHEN v <> '' THEN v END), count(*) FILTER (WHERE v <> ''), \
             sum(CASE WHEN {ok} THEN num END ORDER BY id), count(*) FILTER (WHERE {ok}), \
             min(CASE WHEN {ok} THEN num END), max(CASE WHEN {ok} THEN num END), count(*) FILTER (WHERE {bad})"
        ),
    )
}

struct Metric {
    value: f64,
    n: usize,
    incompatible: usize,
}

/// Reads the eight metric columns starting at `at` (see [`metric_columns`]).
fn metric_at(row: &duckdb::Row<'_>, at: usize, metric: &str, field: bool) -> duckdb::Result<Metric> {
    let count = row.get::<_, i64>(at)? as usize;
    let distinct = row.get::<_, i64>(at + 1)? as usize;
    let nonempty = row.get::<_, i64>(at + 2)? as usize;
    let sum = row.get::<_, Option<f64>>(at + 3)?.unwrap_or(0.0);
    let valid = row.get::<_, i64>(at + 4)? as usize;
    let min = row.get::<_, Option<f64>>(at + 5)?;
    let max = row.get::<_, Option<f64>>(at + 6)?;
    let incompatible = row.get::<_, i64>(at + 7)? as usize;
    Ok(match (metric, field) {
        ("count", _) => Metric { value: count as f64, n: count, incompatible: 0 },
        (_, false) => Metric { value: 0.0, n: 0, incompatible: 0 },
        ("distinct", true) => Metric { value: distinct as f64, n: nonempty, incompatible: 0 },
        (metric, true) => Metric {
            value: match metric {
                "sum" => sum,
                "avg" => {
                    if valid == 0 {
                        0.0
                    } else {
                        sum / valid as f64
                    }
                }
                "min" => min.unwrap_or(0.0),
                "max" => max.unwrap_or(0.0),
                _ => 0.0,
            },
            n: valid,
            incompatible,
        },
    })
}

fn series_of(scope: &Scope, spec: &SeriesSpec) -> Result<SeriesResult> {
    let limit = spec.limit.unwrap_or(10).min(500);
    let field = spec.field.as_deref();
    let mut names = false;
    if spec.chart == "terms" && spec.metric == "count" {
        let key_column = field.unwrap_or("level");
        let (key, time) = key_of(scope, key_column, false, &mut names)?;
        let from = scope.from(names);
        let mut counted = Vec::new();
        for (key, n) in rows(
            scope.session,
            &format!("SELECT k, count(*) FROM (SELECT {key} AS k FROM {from} WHERE {}) GROUP BY k", scope.cond),
            |r| Ok((key_text(r, 0, time)?, r.get::<_, i64>(1)? as usize)),
        )? {
            let key = key?;
            counted.push((serde_json::to_string(&key).map_err(err)?, key, n));
        }
        counted.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        counted.truncate(limit);
        return Ok(SeriesResult {
            kind: "terms".into(),
            unit: "number".into(),
            interval_ms: 0,
            x: counted
                .iter()
                .map(|(_, value, _)| Value::from(value.clone().unwrap_or_else(|| "(vazio)".into())))
                .collect(),
            x_values: counted.iter().map(|(_, value, _)| value.clone()).collect(),
            series: vec![SeriesData {
                name: key_column.into(),
                samples: counted.iter().map(|(_, _, n)| *n).collect(),
                points: counted.iter().map(|(_, _, n)| *n as f64).collect(),
            }],
            incompatible_units: 0,
        });
    }
    let value = match field {
        Some(f) => Some(text_of(scope, f, false, &mut names)?),
        None => None,
    };
    let from = scope.from(names);
    let mut dominant_failure = None;
    let unit = crate::analysis::series_unit(spec, |_| {
        let value = value.as_deref().unwrap_or("NULL");
        let sampled = rows(
            scope.session,
            &format!(
                "SELECT u FROM (SELECT id, li_unit({value}) AS u FROM {from} WHERE {}) \
                 WHERE u IS NOT NULL ORDER BY id LIMIT 500",
                scope.cond
            ),
            |r| r.get::<_, i32>(0),
        );
        let mut votes = [0usize; 5];
        match sampled {
            Ok(units) => units.into_iter().for_each(|u| votes[(u as usize).min(4)] += 1),
            Err(e) => dominant_failure = Some(e),
        }
        crate::analysis::dominant_of(votes)
    });
    if let Some(e) = dominant_failure {
        return Err(e);
    }
    let expected = crate::analysis::expected_unit(spec, &unit);
    let splits: Vec<String> = match &spec.split {
        Some(column) => {
            let split = text_of(scope, column, false, &mut names)?;
            let from = scope.from(names);
            let mut counted = rows(
                scope.session,
                &format!(
                    "SELECT s, count(*) FROM (SELECT {split} AS s FROM {from} WHERE {}) WHERE s <> '' GROUP BY s",
                    scope.cond
                ),
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )?;
            counted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            counted.into_iter().take(6).map(|(s, _)| s).collect()
        }
        None => Vec::new(),
    };
    let split_names: Vec<String> = if splits.is_empty() {
        vec![spec.field.clone().unwrap_or_else(|| "eventos".into())]
    } else {
        splits.clone()
    };
    let (metric_inner, metric_outer) = metric_columns(value.as_deref(), expected);

    if spec.chart == "terms" {
        let key_column = spec.field.clone().unwrap_or_else(|| "level".into());
        let (key, time) = key_of(scope, &key_column, false, &mut names)?;
        let from = scope.from(names);
        let mut items = Vec::new();
        let mut incompatible = 0;
        for (key, metric) in rows(
            scope.session,
            &format!(
                "SELECT k, {metric_outer} FROM (SELECT id, {key} AS k, {metric_inner} FROM {from} WHERE {}) GROUP BY k",
                scope.cond
            ),
            |r| Ok((key_text(r, 0, time)?, metric_at(r, 1, &spec.metric, field.is_some())?)),
        )? {
            incompatible += metric.incompatible;
            items.push((key?, metric.value, metric.n));
        }
        items.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        items.truncate(limit);
        return Ok(SeriesResult {
            kind: "terms".into(),
            unit,
            interval_ms: 0,
            x: items
                .iter()
                .map(|(k, _, _)| Value::from(k.clone().unwrap_or_else(|| "(vazio)".into())))
                .collect(),
            x_values: items.iter().map(|(key, _, _)| key.clone()).collect(),
            series: vec![SeriesData {
                name: split_names[0].clone(),
                samples: items.iter().map(|(_, _, n)| *n).collect(),
                points: items.into_iter().map(|(_, v, _)| v).collect(),
            }],
            incompatible_units: incompatible,
        });
    }

    let bounds = rows(
        scope.session,
        &format!("SELECT min(ts), max(ts) FROM {from} WHERE {}", scope.cond),
        |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?)),
    )?;
    let Some((Some(tmin), Some(tmax))) = bounds.first().copied() else {
        return Ok(SeriesResult {
            kind: "time".into(),
            unit,
            interval_ms: 0,
            x: vec![],
            x_values: vec![],
            series: vec![],
            incompatible_units: 0,
        });
    };
    let (interval, n_buckets) = crate::analysis::series_interval(spec, tmin, tmax);
    let (name_sql, keep) = match &spec.split {
        Some(column) => {
            let split = text_of(scope, column, false, &mut names)?;
            let list = splits.iter().map(|s| lit(s)).collect::<Vec<_>>().join(", ");
            let keep = if splits.is_empty() {
                "FALSE".to_string()
            } else {
                format!("COALESCE({split}, '') IN ({list})")
            };
            (format!("COALESCE({split}, '')"), keep)
        }
        None => (lit(&split_names[0]), "TRUE".to_string()),
    };
    let from = scope.from(names);
    let mut accs: Vec<HashMap<String, Metric>> = (0..n_buckets).map(|_| HashMap::new()).collect();
    let mut incompatible = 0;
    for (b, name, metric) in rows(
        scope.session,
        &format!(
            "SELECT b, name, {metric_outer} FROM (SELECT id, (ts - ({tmin})) // {interval} AS b, {name_sql} AS name, \
             {metric_inner} FROM {from} WHERE ({}) AND ts IS NOT NULL AND {keep}) GROUP BY b, name",
            scope.cond
        ),
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                metric_at(r, 2, &spec.metric, field.is_some())?,
            ))
        },
    )? {
        if b < 0 || b as usize >= n_buckets {
            continue;
        }
        incompatible += metric.incompatible;
        accs[b as usize].insert(name, metric);
    }
    Ok(SeriesResult {
        kind: "time".into(),
        unit,
        interval_ms: interval,
        x: (0..n_buckets)
            .map(|b| Value::from(tmin.saturating_add((b as i64).saturating_mul(interval))))
            .collect(),
        x_values: vec![],
        series: split_names
            .iter()
            .map(|name| SeriesData {
                name: name.clone(),
                samples: accs.iter().map(|m| m.get(name).map(|a| a.n).unwrap_or(0)).collect(),
                points: accs.iter().map(|m| m.get(name).map(|a| a.value).unwrap_or(0.0)).collect(),
            })
            .collect(),
        incompatible_units: incompatible,
    })
}

// ---------------------------------------------------------------- overview

/// `insights::overview` over the engine's columns.
pub(crate) fn overview(src: &Source, pfs: &[PreparedFilter]) -> Option<Overview> {
    with(src, |session| overview_of(&scope(session, src, pfs)?, src))
}

const ERROR_LEVEL: &str = "level IN ('Erro', 'Crítico')";

fn overview_of(scope: &Scope, src: &Source) -> Result<Overview> {
    let from = scope.from(false);
    let cond = &scope.cond;
    let mut out = crate::insights::empty_overview();
    let totals = rows(
        scope.session,
        &format!(
            "SELECT count(*), count(*) FILTER (WHERE {ERROR_LEVEL}), count(*) FILTER (WHERE level = 'Aviso'), \
             count(*) FILTER (WHERE ts IS NULL), min(ts), max(ts) FROM {from} WHERE {cond}"
        ),
        |r| {
            Ok((
                r.get::<_, i64>(0)? as usize,
                r.get::<_, i64>(1)? as usize,
                r.get::<_, i64>(2)? as usize,
                r.get::<_, i64>(3)? as usize,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, Option<i64>>(5)?,
            ))
        },
    )?;
    let (total, errors, warnings, undated, start, end) = totals[0];
    out.total = total;
    out.errors = errors;
    out.warnings = warnings;
    out.undated = undated;
    out.start = start;
    out.end = end;
    out.levels = rows(
        scope.session,
        &format!("SELECT level, count(*) FROM {from} WHERE {cond} GROUP BY level"),
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize)),
    )?
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    // The first 10 000 origins (in file order) are counted.
    let sources: HashMap<String, usize> = rows(
        scope.session,
        &format!(
            "SELECT src, count(*) FROM (SELECT id, CASE WHEN source = '' THEN 'Sem origem' ELSE source END AS src \
             FROM {from} WHERE {cond}) GROUP BY src ORDER BY min(id) LIMIT 10000"
        ),
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize)),
    )?
    .into_iter()
    .collect();
    // The first 20 000 patterns (in file order) are kept; the rest only counts as limited.
    let mut patterns = rows(
        scope.session,
        &format!(
            "SELECT pat, count(*), count(*) FILTER (WHERE {ERROR_LEVEL}), min(ts), max(ts), min(id) \
             FROM {from} WHERE {cond} GROUP BY pat ORDER BY min(id) LIMIT 20001"
        ),
        |r| {
            let mut example = Event::empty();
            example.id = r.get::<_, i64>(5)? as usize;
            Ok(Pattern {
                pattern: r.get(0)?,
                count: r.get::<_, i64>(1)? as usize,
                errors: r.get::<_, i64>(2)? as usize,
                first: r.get(3)?,
                last: r.get(4)?,
                example,
            })
        },
    )?;
    if patterns.len() > 20_000 {
        patterns.truncate(20_000);
        out.patterns_limited = true;
    }
    let latency = latency_of(scope, from)?;
    let mut failure = None;
    let mut out = crate::insights::finish_overview(out, patterns, sources, latency, |buckets, _, start, width| {
        let counted = rows(
            scope.session,
            &format!(
                "SELECT (ts - ({start})) // {width} AS b, count(*), count(*) FILTER (WHERE {ERROR_LEVEL}) \
                 FROM {from} WHERE ({cond}) AND ts IS NOT NULL GROUP BY b"
            ),
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as usize, r.get::<_, i64>(2)? as usize)),
        );
        match counted {
            Ok(counted) => {
                for (b, n, e) in counted {
                    if let Some(bucket) = usize::try_from(b).ok().and_then(|b| buckets.get_mut(b)) {
                        bucket.count += n;
                        bucket.errors += e;
                    }
                }
            }
            Err(e) => failure = Some(e),
        }
    });
    if let Some(e) = failure {
        return Err(e);
    }
    for pattern in &mut out.patterns {
        pattern.example = crate::insights::pattern_example(src.event(pattern.example.id));
    }
    Ok(out)
}

/// Streams the latency candidates of events carrying any of them, in order.
fn latency_of(scope: &Scope, from: &str) -> Result<crate::insights::LatencySample> {
    let mut sample = crate::insights::LatencySample::default();
    let present: Vec<(&str, String)> = crate::insights::LATENCY_FIELDS
        .iter()
        .filter_map(|f| scope.session.schema.fields.get(*f).map(|e| (*f, e.clone())))
        .collect();
    if present.is_empty() {
        return Ok(sample);
    }
    let columns = present.iter().map(|(_, e)| e.as_str()).collect::<Vec<_>>().join(", ");
    let any = present
        .iter()
        .map(|(_, e)| format!("li_nkey({e}) IS NOT NULL"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let conn = scope.session.conn()?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {columns} FROM {from} WHERE ({}) AND ({any}) ORDER BY id",
            scope.cond
        ))
        .map_err(err)?;
    let mut rows = stmt.query([]).map_err(err)?;
    let mut values: Vec<Option<String>> = vec![None; present.len()];
    while let Some(row) = rows.next().map_err(err)? {
        for (i, slot) in values.iter_mut().enumerate() {
            *slot = row.get(i).map_err(err)?;
        }
        sample.push(|k| present.iter().position(|(f, _)| *f == k).and_then(|i| values[i].clone()));
    }
    Ok(sample)
}

// ---------------------------------------------------------------- comparison

/// `insights::compare` over the engine's columns.
pub(crate) fn compare(src: &Source, pfs: &[PreparedFilter], before: &Period, after: &Period) -> Option<Comparison> {
    with(src, |session| {
        let scope = scope(session, src, pfs)?;
        let from = scope.from(false);
        let a = format!("(ts BETWEEN {} AND {})", before.start, before.end);
        let b = format!("(ts BETWEEN {} AND {})", after.start, after.end);
        let inner = format!(
            "SELECT id, pat, {ERROR_LEVEL} AS err, {a} AS a, {b} AS b FROM {from} \
             WHERE ({}) AND ts IS NOT NULL AND ({a} OR {b})",
            scope.cond
        );
        let totals = rows(
            session,
            &format!(
                "SELECT count(*) FILTER (WHERE a), count(*) FILTER (WHERE b), count(*) FILTER (WHERE a AND err), \
                 count(*) FILTER (WHERE b AND err) FROM ({inner})"
            ),
            |r| {
                Ok((
                    r.get::<_, i64>(0)? as usize,
                    r.get::<_, i64>(1)? as usize,
                    r.get::<_, i64>(2)? as usize,
                    r.get::<_, i64>(3)? as usize,
                ))
            },
        )?;
        let (before_total, after_total, before_errors, after_errors) = totals[0];
        let mut groups = rows(
            session,
            &format!(
                "SELECT pat, count(*) FILTER (WHERE a), count(*) FILTER (WHERE b), min(id) FROM ({inner}) \
                 GROUP BY pat ORDER BY min(id) LIMIT 20001"
            ),
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)? as usize,
                    r.get::<_, i64>(2)? as usize,
                    r.get::<_, i64>(3)? as usize,
                ))
            },
        )?;
        let limited = groups.len() > 20_000;
        groups.truncate(20_000);
        let out = Comparison {
            before_total,
            after_total,
            before_errors,
            after_errors,
            changes: vec![],
            limited,
        };
        Ok(crate::insights::finish_compare(out, groups, |id| {
            crate::insights::pattern_example(src.event(id))
        }))
    })
}

// ---------------------------------------------------------------- pivot

/// `analysis::pivot_stream` over light events: only the columns the pivot
/// reads, streamed in file order with the values `Event::col_str` returns.
pub(crate) fn pivot(
    src: &Source,
    pfs: &[PreparedFilter],
    spec: &crate::analysis::PivotSpec,
) -> Option<crate::analysis::PivotResult> {
    with(src, |session| {
        let scope = scope(session, src, pfs)?;
        let mut columns: Vec<String> = spec
            .rows
            .iter()
            .chain(&spec.cols)
            .chain(spec.values.iter().map(|v| &v.column))
            .cloned()
            .collect();
        columns.sort();
        columns.dedup();
        light_events(&scope, &columns, |events| crate::analysis::pivot_stream(events, spec))
    })
}

/// Where a light event keeps one requested column.
enum Slot {
    Timestamp,
    Id,
    Source,
    Level,
    Code,
    Message,
    Name,
    Description,
    EventRef,
    Field(String),
}

fn light_events<T>(
    scope: &Scope,
    columns: &[String],
    consume: impl FnOnce(&mut dyn Iterator<Item = Event>) -> T,
) -> Result<T> {
    let mut names = false;
    let mut select = vec!["id".to_string(), "ts".to_string()];
    let mut slots = Vec::new();
    for column in columns {
        let slot = match column.as_str() {
            "timestamp" => Slot::Timestamp,
            "id" => Slot::Id,
            "source" => Slot::Source,
            "level" => Slot::Level,
            "code" => Slot::Code,
            "message" => Slot::Message,
            "name" => Slot::Name,
            "description" => Slot::Description,
            "event_ref" => Slot::EventRef,
            // Absent roles would be recomputed from the partial event.
            other if other.starts_with('@') && crate::entities::role_of_column(other).is_some() => {
                return Err("Colunas canônicas exigem o evento completo.".into());
            }
            other => Slot::Field(other.to_string()),
        };
        if !matches!(slot, Slot::Timestamp | Slot::Id) {
            let value = scope
                .session
                .schema
                .column(column, &mut names)
                .ok_or("Coluna disponível apenas no texto bruto.")?;
            select.push(value);
        }
        slots.push(slot);
    }
    let sql = format!(
        "SELECT {} FROM {} WHERE {} ORDER BY id",
        select.join(", "),
        scope.from(names),
        scope.cond
    );
    let (sender, receiver) = std::sync::mpsc::sync_channel::<Event>(8192);
    std::thread::scope(|threads| {
        let producer = threads.spawn(move || -> Result<()> {
            let conn = scope.session.conn()?;
            let mut stmt = conn.prepare(&sql).map_err(err)?;
            let mut rows = stmt.query([]).map_err(err)?;
            while let Some(row) = rows.next().map_err(err)? {
                let mut ev = Event::empty();
                ev.id = row.get::<_, i64>(0).map_err(err)? as usize;
                ev.timestamp = row.get(1).map_err(err)?;
                let mut column = 2;
                for slot in &slots {
                    if matches!(slot, Slot::Timestamp | Slot::Id) {
                        continue;
                    }
                    let value: Option<String> = row.get(column).map_err(err)?;
                    column += 1;
                    let text = value.clone().unwrap_or_default();
                    match slot {
                        Slot::Source => ev.source = text,
                        Slot::Level => ev.level = text,
                        Slot::Code => ev.code = text,
                        Slot::Message => ev.message = text,
                        Slot::Name => ev.name = text,
                        Slot::Description => ev.description = text,
                        Slot::EventRef => ev.event_ref = text,
                        Slot::Field(name) => {
                            if let Some(value) = value {
                                ev.fields.insert(name.clone(), Value::String(value));
                            }
                        }
                        Slot::Timestamp | Slot::Id => {}
                    }
                }
                if sender.send(ev).is_err() {
                    break;
                }
            }
            Ok(())
        });
        let result = consume(&mut receiver.iter());
        drop(receiver);
        producer
            .join()
            .map_err(|_| "Falha ao ler o motor de consultas.".to_string())??;
        Ok(result)
    })
}

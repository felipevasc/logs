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
use serde_json::Value;
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

/// Runs `f` on the source's session; errors fall back to the line engine.
fn with<T>(src: &Source, f: impl FnOnce(&Session) -> Result<T>) -> Option<T> {
    let session = session(src.idx, src.codes, src.system, src.derived)?;
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
    let mut stmt = conn.prepare(sql).map_err(err)?;
    let mut ids = Vec::new();
    for batch in stmt.query_arrow([]).map_err(err)? {
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
    let mut ids = read_ids(session, &format!("SELECT id FROM {from} WHERE {} ORDER BY id", plan.sql))?;
    drop(plan.tests);
    for &i in &plan.lines {
        let other = crate::query::scan_matches(src.idx, std::slice::from_ref(&pfs[i].f), src.codes, src.system, src.derived);
        crate::operations::check()?;
        ids = intersect(&ids, &other);
    }
    if !plan.verify.is_empty() {
        let checks: Vec<&PreparedFilter> = plan.verify.iter().map(|&i| &pfs[i]).collect();
        let generation = crate::operations::current_generation();
        let kept: Vec<Vec<usize>> = ids
            .par_chunks(4096)
            .map(|chunk| {
                let mut out = Vec::new();
                for &id in chunk {
                    if crate::operations::cancelled_for(generation) {
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
    if plan.exact() && !costly(&plan.sql) {
        return Ok(Scope {
            session,
            cond: plan.sql,
            names: plan.names,
            _tests: plan.tests,
            _selection: None,
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
        });
    }
    let ids = if plan.exact() {
        let from = if plan.names { "evn" } else { "ev" };
        read_ids(session, &format!("SELECT id FROM {from} WHERE {}", plan.sql))?
    } else {
        drop(plan);
        matching_ids(session, src, pfs)?
    };
    let selection = selection(session, &ids)?;
    if let Some(key) = key {
        session.cache_selection(key, Arc::clone(&selection));
    }
    Ok(Scope {
        session,
        cond: format!("id IN (SELECT id FROM {})", selection.name),
        names: false,
        _tests: Tests::default(),
        _selection: Some(selection),
    })
}

fn selection(session: &Session, ids: &[usize]) -> Result<Arc<Selection>> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let conn = session.conn()?;
    for unused in session.take_garbage() {
        let _ = conn.execute_batch(&format!("DROP TABLE IF EXISTS {unused}"));
    }
    let name = format!("sel_{}", NEXT.fetch_add(1, Ordering::Relaxed));
    conn.execute_batch(&format!("CREATE TABLE {name} (id BIGINT)")).map_err(err)?;
    let selection = Arc::new(Selection {
        name: name.clone(),
        garbage: session.garbage(),
    });
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
    with(src, |session| {
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
    with(src, |session| stats_of(&scope(session, src, pfs)?))
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
        "SELECT {} FROM (SELECT {} FROM {from} WHERE {}) GROUP BY k",
        outer.join(", "),
        inner.join(", "),
        scope.cond
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

pub(crate) fn aggregate(src: &Source, pfs: &[PreparedFilter], group: &str, specs: &[AggSpec]) -> Option<AggResult> {
    with(src, |session| aggregate_of(&scope(session, src, pfs)?, group, specs))
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
    with(src, |session| {
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

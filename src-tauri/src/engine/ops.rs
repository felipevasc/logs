//! Operations answered by the engine. Unavailable/unsupported capabilities
//! permit explicit recovery; execution and resource failures are propagated.
use super::sql::lit;
use super::udf::Tests;
use super::Session;
use crate::analysis::{SeriesData, SeriesResult, SeriesSpec, UnitKind};
use crate::insights::{Comparison, Overview, Pattern, Period};
use crate::model::{class_label, ts_to_iso, CodesConfig, Event};
use crate::query::{
    build_agg_result, build_stats, is_meta_column, sort_levels, stats_layout, Acc, AggResult, AggSpec,
    ExplorerSnapshot, PreparedFilter, QueryResult, Stats,
};
use crate::sources::{event_at, CompiledDerived, FileIndex};
use duckdb::arrow::array::{Array, Int64Array, Int32Array, Float64Array, StringArray, LargeStringArray, StringViewArray, StructArray};
use duckdb::arrow::datatypes::{DataType, Field, Schema};
use duckdb::arrow::record_batch::RecordBatch;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap};
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

fn ready_session(src: &Source, base_allowed: bool) -> Result<Option<Arc<Session>>> {
    if base_allowed && !src.derived.is_empty() {
        if let Some(base) = super::session_checked(src.idx, src.codes, src.system, &[])? { return Ok(Some(base)); }
    }
    super::session_checked(src.idx, src.codes, src.system, src.derived)
}

/// Unavailable/unsupported permits explicit recovery. A failed SQL operation
/// (including cancellation, spill or selection limits) must never trigger a
/// second unbounded line scan or publish an empty success response.
fn analytics_with<T>(src: &Source, base_allowed: bool, f: impl FnOnce(&Session) -> Result<T>) -> Result<Option<T>> {
    crate::operations::check()?;
    let Some(session) = ready_session(src, base_allowed)? else {
        crate::operations::progress("analytics-recovery", "Índice indisponível; verificando arquivo", 0, 0, 0);
        return Ok(None);
    };
    session.collect_garbage()?;
    let _names = session.names_guard(src.codes, src.system)?;
    let result = f(&session);
    crate::operations::check()?;
    match result {
        Ok(value) => { session.collect_garbage()?; Ok(Some(value)) },
        Err(error) if matches!(error.as_str(), "Coluna disponível apenas no texto bruto." | "Colunas canônicas exigem o evento completo." | "Ordenação pelo texto bruto.") => {
            crate::operations::progress("analytics-recovery", "Campo bruto; verificando arquivo", 0, 0, 0);
            Ok(None)
        },
        Err(error) => Err(error),
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

/// Replace a planner token only outside SQL string literals. A user-supplied
/// field value may happen to contain the same text as an internal placeholder.
fn replace_sql_marker(sql: &str, marker: &str, replacement: &str) -> Option<String> {
    let mut out = String::with_capacity(sql.len());
    let bytes = sql.as_bytes();
    let (mut at, mut copied, mut changed) = (0, 0, false);
    while at < bytes.len() {
        if bytes[at] == b'\'' {
            at += 1;
            while at < bytes.len() {
                if bytes[at] == b'\'' {
                    at += 1;
                    if at < bytes.len() && bytes[at] == b'\'' { at += 1; }
                    else { break; }
                } else { at += 1; }
            }
        } else if bytes[at..].starts_with(marker.as_bytes()) {
            out.push_str(&sql[copied..at]);
            out.push_str(replacement);
            at += marker.len();
            copied = at;
            changed = true;
        } else { at += 1; }
    }
    if !changed { return None; }
    out.push_str(&sql[copied..]);
    Some(out)
}

/// Resolve selective field equalities and complete free-text terms, including
/// effective names/descriptions.
/// Narrow terms become one candidate membership predicate: this avoids the
/// old million-row enrichment + MARK join even when only ten records match.
fn resolve_free(session: &Session, src: &Source, sql: &str, tests: &Tests) -> Result<(String, Vec<Arc<Selection>>, bool)> {
    let mut sql = sql.to_string();
    let mut held = Vec::new();
    let mut names = false;
    for term in &tests.hex_fields {
        if replace_sql_marker(&sql, &term.marker, "TRUE").is_none() { continue; }
        let replacement = match exact_hex_selection(session, src, term)? {
            Some(selection) => {
                let predicate = selection.predicate();
                held.push(selection);
                predicate
            }
            None => term.fallback_sql.clone(),
        };
        sql = replace_sql_marker(&sql, &term.marker, &replacement).expect("planner marker found");
    }
    for term in &tests.free {
        if replace_sql_marker(&sql, &term.marker, "TRUE").is_none() { continue; }
        let replacement = match free_selection(session, src, term)? {
            Some(selection) => {
                let predicate = selection.predicate();
                held.push(selection);
                predicate
            }
            None => {
                names = true;
                format!("(contains(vals, {}) OR {})", lit(&term.needle), term.names_sql)
            }
        };
        sql = replace_sql_marker(&sql, &term.marker, &replacement).expect("planner marker found");
    }
    Ok((sql, held, names))
}

fn exact_hex_selection(session: &Session, src: &Source, term: &super::udf::HexField) -> Result<Option<Arc<Selection>>> {
    let gate = crate::analysis_runtime::indexed_gate(src.idx)?;
    // A selective lookup must not replace cheap vectorized filtering with an
    // unbounded amount of record hydration. Dense terms retain native SQL.
    const LIMIT: usize = 4_096;
    let filter = &term.filter;
    let key = format!("exact-hex#{}", serde_json::to_string(&(&filter.f.column, &filter.f.value)).map_err(err)?);
    if let Some(found) = session.cached_selection(&key) { return Ok(Some(found)); }
    #[cfg(test)]
    EXACT_HEX_CANDIDATE_PROBES.with(|probes| probes.set(probes.get() + 1));
    let Some(candidates) = session.exact_hex_candidates(&term.word, LIMIT)? else { return Ok(None) };
    let mut confirmed = Vec::with_capacity(candidates.len());
    for id in candidates {
        crate::operations::check()?;
        if gate.as_ref().is_some_and(|gate| !gate.allows_known_row(id)) { continue; }
        if crate::query::matches_indexed(&src.event(id), filter) { confirmed.push(id); }
    }
    let selected = selection(session, &confirmed)?;
    session.cache_selection(key, Arc::clone(&selected));
    Ok(Some(selected))
}

#[cfg(test)]
thread_local! {
    static FREE_CANDIDATE_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static EXACT_HEX_CANDIDATE_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn free_selection(session: &Session, src: &Source, term: &super::udf::FreeText) -> Result<Option<Arc<Selection>>> {
    let gate = crate::analysis_runtime::indexed_gate(src.idx)?;
    let needle = &term.needle;
    let key = format!("free-v2#{needle}");
    if let Some(found) = session.cached_selection(&key) {
        return Ok(Some(found));
    }
    // The text index covers stored values AND intrinsic names/descriptions.
    // Catalog overrides are a separate, small source/code relation.
    #[cfg(test)]
    FREE_CANDIDATE_PROBES.with(|probes| probes.set(probes.get() + 1));
    let Some(mut candidates) = session.free_candidates(needle, FREE_LIMIT)? else { return Ok(None) };
    if !session.baked {
        let catalog = read_ids(session, &catalog_candidates_sql(&term.names_sql))?;
        if catalog.len() > FREE_LIMIT { return Ok(None); }
        candidates.extend(catalog);
        candidates.sort_unstable();
        candidates.dedup();
        if candidates.len() > FREE_LIMIT { return Ok(None); }
    }
    let found = if candidates.len() <= 4_096 {
        // Random access through existing line offsets avoids scanning columnar
        // row IDs/text to confirm a tiny candidate set. The canonical matcher
        // also avoids allocating a second joined copy of a very wide record.
        let mut confirmed = Vec::with_capacity(candidates.len());
        for id in candidates {
            crate::operations::check()?;
            if gate.as_ref().is_some_and(|gate| !gate.allows_known_row(id)) { continue; }
            let event = src.event(id);
            if crate::querylang::any_value(&event, &|value| {
                crate::query::ci_contains_bytes(value.as_bytes(), needle.as_bytes())
            })
            { confirmed.push(id); }
        }
        selection(session, &confirmed)?
    } else {
        let pending = selection(session, &candidates)?;
        selection_query(session, &format!(
            "SELECT id FROM evn WHERE id IN (SELECT id FROM {}) AND (contains(vals, {}) OR {}) ORDER BY id",
            pending.name, lit(needle), term.names_sql
        ))?
    };
    session.cache_selection(key, Arc::clone(&found));
    Ok(Some(found))
}

fn catalog_candidates_sql(names_sql: &str) -> String {
    // A correlated EXISTS with the exact/wildcard OR makes DuckDB retain the
    // entire event side in a delimiter join, even for an empty catalog. Two
    // ordinary equality semi joins build from the small catalog instead.
    // An event may occur in both branches; the caller deduplicates candidates
    // and verifies the effective catalog precedence with the canonical matcher.
    // Hitting the bound (including duplicates) conservatively uses the general
    // query path, so LIMIT never publishes an incomplete candidate selection.
    format!(
        "SELECT id FROM (\
         SELECT ev.id FROM ev SEMI JOIN \
         (SELECT source, code FROM enr WHERE source <> '*' AND ({names_sql})) AS matched \
         ON matched.source=ev.source AND matched.code=ev.code \
         UNION ALL \
         SELECT ev.id FROM ev SEMI JOIN \
         (SELECT code FROM enr WHERE source='*' AND ({names_sql})) AS matched \
         ON matched.code=ev.code) LIMIT {}",
        FREE_LIMIT + 1
    )
}

/// Table holding the ids a filter list selects; dropped with its last user.
pub(crate) struct Selection {
    name: String,
    known_empty: bool,
    single_id: Option<usize>,
    rows: AtomicU64,
    garbage: Arc<parking_lot::Mutex<Vec<SelectionGarbage>>>,
    pool: Arc<crate::case_work_budget::Pool>,
    credit: parking_lot::Mutex<Option<crate::case_work_budget::Lease>>,
}

/// A deleted selection still occupies its logical quota until DROP succeeds.
#[derive(Debug)]
pub(crate) struct SelectionGarbage {
    pub name: String,
    pub credit: Option<crate::case_work_budget::Lease>,
}
impl SelectionGarbage {
    #[cfg(test)]
    fn new(name: impl Into<String>) -> Self { Self { name: name.into(), credit: None } }
}
impl std::fmt::Display for SelectionGarbage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.name.fmt(f) }
}

impl Selection {
    pub(crate) fn name(&self) -> &str { &self.name }
    pub(crate) fn accounted_bytes(&self) -> u64 { self.rows.load(Ordering::Relaxed).saturating_mul(8) }
    pub(crate) fn same_owner(&self, other: &Selection) -> bool { self.pool.same_owner(&other.pool) }
    pub(crate) fn releases_pressure(&self, pool: &crate::case_work_budget::Pool, pressure: crate::case_work_budget::Pressure) -> bool {
        pool.releases_pressure(&self.pool, pressure)
    }
    fn reserve_rows(&self, session: &Session, rows: u64) -> Result<()> {
        let bytes = usize::try_from(rows.checked_mul(8).ok_or(crate::case_work_budget::WORK_BUSY)?).map_err(err)?;
        let held = self.credit.lock().as_ref().map_or(0, |credit| credit.bytes());
        if bytes <= held { return Ok(()); }
        let reserve = || crate::case_cache::reserve_work(&self.pool, bytes - held);
        let credit = match reserve() {
            Ok(credit) => credit,
            Err(error) if error == crate::case_work_budget::WORK_BUSY => {
                let Some(pressure) = self.pool.pressure(bytes - held) else { return Err(error); };
                super::trim_inactive_selection_caches(Some(session), Some((&self.pool, pressure)))?;
                reserve()?
            },
            Err(error) => return Err(error),
        };
        let mut existing = self.credit.lock();
        match existing.as_mut() { Some(existing) => existing.merge(credit)?, None => *existing = Some(credit) }
        Ok(())
    }
    fn predicate(&self) -> String {
        if self.known_empty { "FALSE".into() }
        else { format!("id IN (SELECT id FROM {})", self.name) }
    }
}

impl Drop for Selection {
    fn drop(&mut self) {
        self.garbage.lock().push(SelectionGarbage {
            name: std::mem::take(&mut self.name), credit: self.credit.get_mut().take(),
        });
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

fn visit_ids(session: &Session, sql: &str, mut visit: impl FnMut(usize) -> Result<bool>) -> Result<()> {
    let conn = session.conn()?;
    let _cancel = interruptible(&conn)?;
    let mut stmt = conn.prepare(sql).map_err(err)?;
    drop(stmt.stream_arrow([]).map_err(err)?);
    while let Some(batch) = stmt.step().map_err(err)? {
        crate::operations::check()?;
        let values = batch.column(0).as_any().downcast_ref::<Int64Array>().ok_or("Identificadores em formato inesperado.")?;
        for &value in values.values().iter() {
            let id = usize::try_from(value).map_err(err)?;
            if !visit(id)? { return Ok(()); }
        }
    }
    crate::operations::check()
}
fn read_ids(session: &Session, sql: &str) -> Result<Vec<usize>> {
    let mut ids = Vec::new();
    visit_ids(session, sql, |id| {
        crate::query::push_collected_id(&mut ids, id)?;
        Ok(true)
    })?;
    Ok(ids)
}

/// The all-ID API is explicitly bounded; analytics and callbacks use the
/// database selection/stream instead of collecting this result.
fn matching_ids(session: &Session, src: &Source, pfs: &[PreparedFilter]) -> Result<Vec<usize>> {
    if pfs.is_empty() { crate::query::check_collected_ids(crate::analysis_runtime::visible_total(src.idx)?)?; }
    let scope = scope(session, src, pfs)?;
    read_ids(session, &format!("SELECT id FROM {} WHERE {} ORDER BY id", scope.from(false), scope.cond))
}

/// Conditions that read long texts on every row (free text, messages,
/// rendered times). Their selection is computed once and reused by the
/// queries of a screen.
fn costly(sql: &str) -> bool {
    ["vals", "message", "li_iso(", "event_ref", "xv[", "(name,", "(description,"]
        .iter()
        .any(|part| sql.contains(part))
}

fn cacheable_filters(pfs: &[PreparedFilter]) -> bool {
    fn stable(expr: &crate::querylang::Expr) -> bool {
        use crate::querylang::{Expr, TermKind};
        match expr {
            Expr::All => true,
            Expr::And(items) | Expr::Or(items) => items.iter().all(stable),
            Expr::Not(inner) => stable(inner),
            Expr::Term(term) => !matches!(term.kind(), TermKind::Event),
        }
    }
    pfs.iter().all(|pf| !matches!(pf.f.op.as_str(), "detection" | "threat_rule" | "finding" | "episode") && pf.expr.as_ref().is_none_or(stable))
}

/// Mandatory visibility is independent of the user predicate and of whether
/// that predicate was compiled exactly or recovered through a selection table.
fn visible_sql(src: &Source, sql: String, tests: &mut Tests) -> Result<String> {
    let Some(gate) = crate::analysis_runtime::indexed_gate(src.idx)? else { return Ok(sql); };
    if gate.is_unrestricted() { return Ok(sql); }
    let predicate = tests.row("id", Arc::new(move |id| {
        let id = usize::try_from(id).map_err(|_| "Identidade de linha negativa na visibilidade.".to_string())?;
        gate.allows(id)
    }));
    Ok(format!("({sql}) AND ({predicate})"))
}

fn complete_selection_key(session: &Session, pfs: &[PreparedFilter]) -> Option<String> {
    // Detection/threat registries are mutable independently of the source;
    // do not reuse those until their revisions become part of this key.
    cacheable_filters(pfs).then(|| {
        let filters: Vec<&crate::query::Filter> = pfs.iter().map(|pf| &pf.f).collect();
        format!("{}#{}", serde_json::to_string(&filters).unwrap_or_default(), session.names_version())
    })
}

/// Use the same eligibility and canonical key for analytics and page hits.
/// Cheap exact plans without markers never materialize a whole selection;
/// preserve their allocation-free key path.
fn complete_selection_key_for_plan(session: &Session, pfs: &[PreparedFilter], plan: &super::sql::Plan) -> Option<String> {
    let may_materialize = !plan.exact() || costly(&plan.sql)
        || !plan.tests.free.is_empty() || !plan.tests.hex_fields.is_empty();
    if may_materialize { complete_selection_key(session, pfs) } else { None }
}

fn scope<'s>(session: &'s Session, src: &Source, pfs: &[PreparedFilter]) -> Result<Scope<'s>> {
    crate::operations::check()?;
    let mut plan = session.schema.plan(pfs);
    let key = complete_selection_key_for_plan(session, pfs, &plan);
    // A complete retained result already includes every required text term.
    // Re-probing those terms first can churn the shared LRU and evict this
    // result before it is used. The Session still binds the key to the full
    // admitted namespace, and visible_sql revalidates mandatory visibility.
    if let Some(found) = key.as_deref().and_then(|key| session.cached_selection(key)) {
        return Ok(Scope {
            session,
            cond: visible_sql(src, found.predicate(), &mut plan.tests)?,
            names: false,
            _tests: plan.tests,
            _selection: Some(found),
            _free: Vec::new(),
        });
    }
    let (sql, free, free_names) = resolve_free(session, src, &plan.sql, &plan.tests)?;
    let sql = visible_sql(src, sql, &mut plan.tests)?;
    if plan.exact() && !costly(&sql) {
        return Ok(Scope {
            session,
            cond: sql,
            names: plan.names || free_names,
            _tests: plan.tests,
            _selection: None,
            _free: free,
        });
    }
    let _build = key.as_deref().map(|key| session.begin_selection(key)).transpose()?;
    if let Some(found) = key.as_deref().and_then(|key| session.cached_selection(key)) {
        return Ok(Scope { session, cond: visible_sql(src, found.predicate(), &mut plan.tests)?, names: false, _tests: plan.tests, _selection: Some(found), _free: Vec::new() });
    }
    let selection = if plan.exact() {
        let from = if plan.names || free_names { "evn" } else { "ev" };
        // Keep a potentially 50M-row selection inside DuckDB, whose buffer
        // manager can spill it, instead of a Rust Vec followed by another copy.
        selection_query(session, &format!("SELECT id FROM {from} WHERE {sql}"))?
    } else {
        verified_selection(session, src, pfs, &plan, &sql, free_names)?
    };
    if let Some(key) = key {
        session.cache_selection(key, Arc::clone(&selection));
    }
    Ok(Scope {
        session,
        cond: selection.predicate(),
        names: false,
        _tests: Tests::default(),
        _selection: Some(selection),
        _free: Vec::new(),
    })
}

enum PageScope<'s> {
    Singleton(crate::query::SelectedPage),
    Planned { scope: Scope<'s>, verify: Vec<usize> },
}

/// Hit-only reuse: never begin or wait for a complete analytics selection.
/// Keep the retained Arc alive through sorting/SQL, including concurrent LRU
/// eviction, and reapply mandatory visibility under the admitted namespace.
fn page_scope<'s>(session: &'s Session, src: &Source, pfs: &[PreparedFilter], first_page: bool) -> Result<PageScope<'s>> {
    crate::operations::check()?;
    let mut plan = session.schema.plan(pfs);
    // Analytics' raw/meta shortcut for `_all` line predicates is not a proof
    // of canonical Event matching: it trims needles and cannot see generated
    // messages or a snapshot's embedded raw text. Do not reuse those results
    // on a page until that separate parity contract has been repaired.
    // Reuse complete results only to remove canonical event confirmation.
    // SQL-exact plans retain their direct predicate: joining a broad selection
    // can cost more than the vectorized predicate the page already had.
    let key = if !plan.verify.is_empty() && plan.lines.is_empty() {
        complete_selection_key_for_plan(session, pfs, &plan)
    } else { None };
    if let Some(found) = key.as_deref().and_then(|key| session.cached_selection(key)) {
        let cond = visible_sql(src, found.predicate(), &mut plan.tests)?;
        crate::operations::check()?;
        return Ok(PageScope::Planned {
            scope: Scope { session, cond, names: false, _tests: plan.tests, _selection: Some(found), _free: Vec::new() },
            verify: Vec::new(),
        });
    }
    // Preserve the first-page singleton shortcut, but only after a complete
    // cache hit has had the chance to skip all new text/hex probes.
    if first_page {
        if let Some(page) = required_hex_page(session, src, pfs, &plan)? {
            return Ok(PageScope::Singleton(page));
        }
    }
    let (sql, free, free_names) = resolve_free(session, src, &plan.sql, &plan.tests)?;
    let sql = visible_sql(src, sql, &mut plan.tests)?;
    // SQL proves the remaining predicates. Conservatively confirm line
    // residuals on the canonical event too, without using the raw shortcut.
    let mut verify = plan.verify;
    verify.extend(plan.lines);
    verify.sort_unstable();
    crate::operations::check()?;
    Ok(PageScope::Planned {
        scope: Scope { session, cond: sql, names: plan.names || free_names, _tests: plan.tests, _selection: None, _free: free },
        verify,
    })
}

fn new_selection(session: &Session, known_empty: bool, single_id: Option<usize>) -> Arc<Selection> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    Arc::new(Selection {
        name: format!("sel_{}", NEXT.fetch_add(1, Ordering::Relaxed)),
        known_empty,
        single_id,
        rows: AtomicU64::new(u64::MAX),
        garbage: session.garbage(),
        pool: crate::case_resources::current_selection_pool(),
        credit: parking_lot::Mutex::new(None),
    })
}

fn selection_query(session: &Session, sql: &str) -> Result<Arc<Selection>> {
    let selection = new_selection(session, false, None);
    let max = crate::resources::selection_bytes() / 8;
    // Preserve the fast INSERT path when immutable source metadata proves a
    // quota-fitting upper bound. Under pressure count the bounded result first,
    // instead of refusing a narrow filter just because its source is large.
    let upper = session.row_upper_bound().filter(|rows| *rows as u64 <= max);
    let reserved = upper.map(|rows| selection.reserve_rows(session, rows as u64));
    let rows = match reserved {
        Some(Ok(())) => upper.map(|rows| rows as u64),
        Some(Err(error)) if error != crate::case_work_budget::WORK_BUSY => return Err(error),
        _ => {
            let conn = session.conn()?;
            let _cancel = interruptible(&conn)?;
            let rows: u64 = conn.query_row(&format!("SELECT count(*) FROM (SELECT id FROM ({sql}) LIMIT {})", max.saturating_add(1)), [], |row| row.get(0)).map_err(err)?;
            check_selection_size(rows)?;
            selection.reserve_rows(session, rows)?;
            Some(rows)
        }
    };
    let bound = rows.unwrap_or(0);
    selection_write(session, &selection, |conn| {
        crate::operations::progress("analytics-select", "Selecionando candidatos", 0, 0, 0);
        // Source is immutable during this admitted session. The unbounded
        // INSERT cannot silently truncate; the preflight proves its upper bound.
        let rows = conn.execute(&format!("INSERT INTO {} {sql}", selection.name), []).map_err(err)? as u64;
        if rows > bound { return Err("A seleção divergiu da fonte imutável admitida.".into()); }
        check_selection_size(rows)?;
        Ok(rows)
    })?;
    Ok(selection)
}

fn selection(session: &Session, ids: &[usize]) -> Result<Arc<Selection>> {
    check_selection_size(ids.len() as u64)?;
    let selection = new_selection(session, ids.is_empty(), if ids.len() == 1 { Some(ids[0]) } else { None });
    selection.reserve_rows(session, ids.len() as u64)?;
    selection_write(session, &selection, |conn| {
        let mut appender = conn.appender(&selection.name).map_err(err)?;
        for chunk in ids.chunks((crate::resources::batch_bytes() / 8).clamp(1, 65536)) {
            crate::operations::check()?;
            append_ids(&mut appender, chunk)?;
        }
        appender.flush().map_err(err)?;
        Ok(ids.len() as u64)
    })?;
    Ok(selection)
}

fn check_selection_size(rows: u64) -> Result<()> {
    if rows.saturating_mul(8) > crate::resources::selection_bytes() {
        Err("A seleção excedeu o orçamento de IDs (LOGINSIGHT_SELECTION_LIMIT_MB). Restrinja os filtros ou aumente o limite.".into())
    } else { Ok(()) }
}

/// The table becomes visible only after complete verification and commit.
/// Cleanup failures retain their pending tables; failed transaction rollback
/// discards the pooled connection instead of reusing an unknown state.
fn selection_write(session: &Session, selection: &Selection, fill: impl FnOnce(&duckdb::Connection) -> Result<u64>) -> Result<()> {
    let mut conn = session.conn()?;
    session.collect_garbage_on(&conn)?;
    let cancel = interruptible(&conn)?;
    conn.execute_batch("BEGIN TRANSACTION").map_err(err)?;
    let result: Result<()> = (|| {
        conn.execute_batch(&format!("CREATE TABLE {} (id BIGINT)", selection.name)).map_err(err)?;
        let rows = fill(&conn)?;
        crate::operations::check()?;
        let bytes = usize::try_from(rows.checked_mul(8).ok_or(crate::case_work_budget::WORK_BUSY)?).map_err(err)?;
        let mut credit = selection.credit.lock();
        if bytes > credit.as_ref().map_or(0, |credit| credit.bytes()) { return Err("Seleção sem reserva de volume lógico.".into()); }
        conn.execute_batch("COMMIT").map_err(err)?;
        if let Some(credit) = credit.as_mut() { credit.resize(bytes)?; }
        selection.rows.store(rows, Ordering::Relaxed);
        Ok(())
    })();
    drop(cancel);
    if result.is_err() && conn.execute_batch("ROLLBACK").is_err() { conn.discard(); }
    result?;
    crate::operations::check()
}

fn append_ids(appender: &mut duckdb::Appender<'_>, ids: &[usize]) -> Result<()> {
    if ids.is_empty() { return Ok(()); }
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
    let array = Int64Array::from_iter_values(ids.iter().map(|&i| i as i64));
    let batch = RecordBatch::try_new(schema, vec![Arc::new(array)]).map_err(err)?;
    appender.append_record_batch(batch).map_err(err)
}

fn verified_selection(session: &Session, src: &Source, pfs: &[PreparedFilter], plan: &super::sql::Plan, sql: &str, free_names: bool) -> Result<Arc<Selection>> {
    let selection = new_selection(session, false, None);
    selection_write(session, &selection, |writer| {
        // A second connection is required: appending on the streaming reader
        // would invalidate its active result. Both share this immutable source.
        let reader = session.conn()?;
        let _cancel = interruptible(&reader)?;
        let from = if plan.names || free_names { "evn" } else { "ev" };
        let mut stmt = reader.prepare(&format!("SELECT id FROM {from} WHERE {sql}")).map_err(err)?;
        crate::operations::progress("analytics-select", "Selecionando candidatos", 0, 0, 0);
        // Arrow's Iterator::next panics on a late fetch error. Start streaming,
        // then use the public fallible step API so no partial result is cached.
        drop(stmt.stream_arrow([]).map_err(err)?);
        let verifier = crate::query::CandidateVerifier::new(src.idx, pfs, &plan.lines, &plan.verify, src.codes, src.system, src.derived)?;
        let cancellation = crate::operations::current_token();
        let mut appender = writer.appender(&selection.name).map_err(err)?;
        let mut examined = 0usize;
        let mut selected = 0u64;
        let mut last_report = std::time::Instant::now();
        while let Some(batch) = stmt.step().map_err(err)? {
            crate::operations::check()?;
            let ids = batch.column(0).as_any().downcast_ref::<Int64Array>().ok_or("Identificadores em formato inesperado.")?;
            let mut from = 0;
            while from < ids.len() {
                let mut to = from;
                let mut bytes = 0usize;
                while to < ids.len() {
                    let id = usize::try_from(ids.value(to)).map_err(err)?;
                    let meta = src.idx.lines.get(id).ok_or("Identificador de candidato fora da fonte.")?;
                    let next = meta.len as usize;
                    if to > from && bytes.saturating_add(next) > crate::resources::batch_bytes() { break; }
                    bytes = bytes.saturating_add(next);
                    to += 1;
                }
                let kept: Vec<usize> = crate::global_scheduler::map(from..to, |at| {
                    if cancellation.cancelled() { return None; }
                    let id = ids.value(at) as usize;
                    verifier.matches(id).then_some(id)
                }).into_iter().flatten().collect();
                crate::operations::check()?;
                selected = selected.saturating_add(kept.len() as u64);
                check_selection_size(selected)?;
                selection.reserve_rows(session, selected)?;
                append_ids(&mut appender, &kept)?;
                examined += to - from;
                if last_report.elapsed() >= std::time::Duration::from_millis(200) {
                    crate::operations::progress("analytics-verify", "Verificando candidatos", examined, 0, selected as usize);
                    last_report = std::time::Instant::now();
                }
                from = to;
            }
        }
        appender.flush().map_err(err)?;
        crate::operations::progress("analytics-verify", "Verificação concluída; publicando seleção", examined, 0, selected as usize);
        Ok(selected)
    })?;
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

struct StreamRow<'a> { batch: &'a StructArray, at: usize }
impl StreamRow<'_> {
    fn get<T: duckdb::types::FromSql>(&self, index: usize) -> Result<T> {
        use duckdb::types::ValueRef;
        let column = self.batch.column(index);
        let value = if column.is_null(self.at) { ValueRef::Null }
        else if let Some(a) = column.as_any().downcast_ref::<Int64Array>() { ValueRef::BigInt(a.value(self.at)) }
        else if let Some(a) = column.as_any().downcast_ref::<Int32Array>() { ValueRef::Int(a.value(self.at)) }
        else if let Some(a) = column.as_any().downcast_ref::<Float64Array>() { ValueRef::Double(a.value(self.at)) }
        else if let Some(a) = column.as_any().downcast_ref::<StringArray>() { ValueRef::Text(a.value(self.at).as_bytes()) }
        else if let Some(a) = column.as_any().downcast_ref::<LargeStringArray>() { ValueRef::Text(a.value(self.at).as_bytes()) }
        else if let Some(a) = column.as_any().downcast_ref::<StringViewArray>() { ValueRef::Text(a.value(self.at).as_bytes()) }
        else { return Err(format!("Tipo de resultado analítico não suportado: {:?}", column.data_type())); };
        T::column_result(value).map_err(err)
    }
}
fn stream_rows(session: &Session, sql: &str, mut visit: impl FnMut(StreamRow<'_>) -> Result<()>) -> Result<()> {
    let conn = session.conn()?;
    let _cancel = interruptible(&conn)?;
    let mut stmt = conn.prepare(sql).map_err(err)?;
    crate::operations::progress("analytics-sql", "Calculando estatísticas", 0, 0, 0);
    drop(stmt.stream_arrow([]).map_err(err)?);
    while let Some(batch) = stmt.step().map_err(err)? {
        crate::operations::check()?;
        for at in 0..batch.len() { visit(StreamRow { batch: &batch, at })?; }
    }
    crate::operations::check()
}
fn stream_key(row: &StreamRow<'_>, index: usize, time: bool) -> Result<Option<String>> {
    if !time { return row.get(index); }
    match row.get::<Option<i64>>(index)? {
        None => Ok(None),
        Some(ts) => {
            let text = ts_to_iso(ts);
            if text.trim().is_empty() { Err("Data fora do intervalo representável.".into()) }
            else { Ok(Some(text)) }
        }
    }
}

#[cfg(test)]
mod bounded_analytics_tests {
    use super::*;
    use parking_lot::{Mutex, RwLock};

    fn session() -> Session {
        Session {
            row_count: None,
            key: "bounded-test".into(), base: Mutex::new(duckdb::Connection::open_in_memory().unwrap()),
            pool: Mutex::new(Vec::new()), schema: super::super::sql::Schema::default(),
            timestamps_non_null: false, baked: false, names: RwLock::new(None),
            names_version: AtomicU64::new(0), selections: Mutex::new(Vec::new()),
            selection_builds: Mutex::new(std::collections::HashSet::new()), selection_changed: parking_lot::Condvar::new(),
            garbage: Arc::new(Mutex::new(Vec::new())), texts: Vec::new(),
            time_indexes: RwLock::new(None), _leases: Vec::new(),
        }
    }

    fn chart(session: &Session, spec: serde_json::Value) -> Result<SeriesResult> {
        let spec: SeriesSpec = serde_json::from_value(spec).unwrap();
        let scope = Scope { session, cond: "TRUE".into(), names: false, _tests: Tests::default(), _selection: None, _free: Vec::new() };
        series_of(&scope, &spec)
    }

    #[test]
    fn series_count_stream_retains_only_top_keys_under_small_value_budget() {
        let session = session();
        let conn = session.conn().unwrap();
        conn.execute_batch("SET threads=1; SET memory_limit='32MB'; \
            CREATE VIEW ev AS SELECT range::BIGINT AS id, repeat('x',64) || lpad(range::VARCHAR,4,'0') AS level FROM range(4096)").unwrap();
        super::super::udf::register(&conn).unwrap();
        drop(conn);
        let result = crate::resources::with_analytics_limit(8192, || chart(&session, serde_json::json!({"chart":"terms","metric":"count","limit":10}))).unwrap();
        assert_eq!(result.x_values, (0..10).map(|i| Some(format!("{}{i:04}", "x".repeat(64)))).collect::<Vec<_>>());
        assert_eq!(result.series[0].samples, vec![1;10]);
        assert!(chart(&session, serde_json::json!({"chart":"terms","metric":"count","limit":0})).unwrap().x.is_empty());
        assert_eq!(chart(&session, serde_json::json!({"chart":"terms","metric":"count","limit":900})).unwrap().x.len(), 500);
    }

    #[test]
    fn series_numeric_terms_count_incompatible_units_below_top_n() {
        let session = session();
        let conn = session.conn().unwrap();
        conn.execute_batch("CREATE TABLE ev(id BIGINT, source VARCHAR); \
            INSERT INTO ev VALUES (0,'1KB'),(1,'2KB'),(2,'3s'),(3,'3s'),(4,'4s')").unwrap();
        super::super::udf::register(&conn).unwrap();
        drop(conn);
        for metric in ["sum", "avg", "min", "max"] {
            for limit in [0, 1, 3] {
                let result = chart(&session, serde_json::json!({"chart":"terms","metric":metric,"field":"source","unit":"bytes","limit":limit})).unwrap();
                assert_eq!(result.incompatible_units, 3);
                if limit > 0 {
                    assert_eq!(result.x_values[0], Some("2KB".into()));
                    assert_eq!(result.series[0].points[0], 2048.0);
                    assert_eq!(result.series[0].samples[0], 1);
                }
                assert_eq!(result.x.len(), limit);
            }
        }
        let distinct = chart(&session, serde_json::json!({"chart":"terms","metric":"distinct","field":"source","unit":"bytes","limit":1})).unwrap();
        assert_eq!(distinct.x_values, vec![Some("1KB".into())]);
        assert_eq!(distinct.incompatible_units, 0);
    }

    #[test]
    fn series_split_stream_keeps_six_raw_key_ties_without_other() {
        let session = session();
        let conn = session.conn().unwrap();
        conn.execute_batch("CREATE TABLE ev(id BIGINT, ts BIGINT, level VARCHAR)").unwrap();
        for (id, key) in ["\n", "\"", "\\", "a", "b", "c", "d", "é"].into_iter().enumerate() {
            conn.execute("INSERT INTO ev VALUES (?,1,?)", duckdb::params![id as i64,key]).unwrap();
        }
        super::super::udf::register(&conn).unwrap();
        drop(conn);
        let result = chart(&session, serde_json::json!({"chart":"time","metric":"count","split":"level"})).unwrap();
        assert_eq!(result.series.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["\n", "\"", "\\", "a", "b", "c"]);
        assert!(result.series.iter().all(|s| s.samples == [1] && s.points == [1.0]));
    }

    #[test]
    fn series_budget_and_sql_errors_do_not_publish_partial_charts() {
        let session = session();
        let conn = session.conn().unwrap();
        conn.execute_batch("CREATE VIEW ev AS SELECT range::BIGINT AS id, repeat('x',4096) AS level FROM range(2)").unwrap();
        super::super::udf::register(&conn).unwrap();
        drop(conn);
        let error = crate::resources::with_analytics_limit(1024, || chart(&session, serde_json::json!({"chart":"terms","metric":"count","limit":1}))).err().unwrap();
        assert!(error.contains("LOGINSIGHT_ANALYTICS_LIMIT_MB"));
        session.conn().unwrap().execute_batch("DROP VIEW ev; CREATE VIEW ev AS SELECT range::BIGINT AS id, \
            CASE WHEN range>=4096 THEN error('chart fetch failure') ELSE range::VARCHAR END AS level FROM range(5000)").unwrap();
        let error = chart(&session, serde_json::json!({"chart":"terms","metric":"count","limit":1})).err().unwrap();
        assert!(error.contains("chart fetch failure"));
        assert_eq!(session.conn().unwrap().query_row("SELECT 42", [], |r| r.get::<_,i64>(0)).unwrap(), 42);
    }

    #[test]
    fn series_cancelled_stream_is_an_error_and_can_retry() {
        let session = session();
        let conn = session.conn().unwrap();
        conn.execute_batch("CREATE TABLE ev(level VARCHAR); INSERT INTO ev VALUES ('a')").unwrap();
        super::super::udf::register(&conn).unwrap();
        drop(conn);
        let token = crate::operations::token(Some("series-top-cancel".into())).unwrap();
        let cancelled = crate::operations::run_with_token(token, || {
            crate::operations::cancel_id("series-top-cancel");
            let error = chart(&session, serde_json::json!({"chart":"terms","metric":"count"})).err().unwrap();
            assert!(error.contains("cancelad"));
        });
        assert!(cancelled.is_err());
        assert_eq!(chart(&session, serde_json::json!({"chart":"terms","metric":"count"})).unwrap().series[0].samples, vec![1]);
    }

    #[test]
    fn series_metric_projection_plans_omit_unrequested_aggregates() {
        let session = session();
        let conn = session.conn().unwrap();
        conn.execute_batch("CREATE TABLE ev(id BIGINT, source VARCHAR, level VARCHAR)").unwrap();
        // Count/distinct must bind and execute without the numeric UDFs.
        for metric in ["count", "distinct"] {
            let (inner, outer) = metric_columns(Some("source"), None, metric);
            let mut statement = conn.prepare(&format!("SELECT k,{outer} FROM (SELECT id,level AS k,{inner} FROM ev) GROUP BY k")).unwrap();
            assert!(statement.query([]).unwrap().next().unwrap().is_none());
        }
        super::super::udf::register(&conn).unwrap();
        for metric in ["count", "distinct", "sum", "avg", "min", "max"] {
            let (inner, outer) = metric_columns(Some("source"), Some(UnitKind::Bytes), metric);
            let plan: String = conn.query_row(&format!("EXPLAIN SELECT k,{outer} FROM (SELECT id,level AS k,{inner} FROM ev) GROUP BY k"), [], |r| r.get(1)).unwrap();
            assert_eq!(plan.contains("count(DISTINCT"), metric == "distinct", "{metric}: {plan}");
            assert_eq!(plan.contains("sum("), matches!(metric, "sum" | "avg"), "{metric}: {plan}");
            assert_eq!(plan.contains("min("), metric == "min", "{metric}: {plan}");
            assert_eq!(plan.contains("max("), metric == "max", "{metric}: {plan}");
            assert_eq!(outer.contains("ORDER BY id"), matches!(metric, "sum" | "avg"));
        }
    }

    #[test]
    fn series_metric_projection_keeps_file_order_samples_and_warning_counts() {
        let session = session();
        let conn = session.conn().unwrap();
        conn.execute_batch("CREATE TABLE ev(id BIGINT, ts BIGINT, source VARCHAR); INSERT INTO ev VALUES \
            (3,1,'3'),(2,1,'-10000000000000000'),(1,1,'1'),(0,1,'10000000000000000'), \
            (4,1,'2KB'),(5,1,''),(6,1,NULL)").unwrap();
        super::super::udf::register(&conn).unwrap();
        drop(conn);
        for (metric, point, samples, warnings) in [
            ("count", 7.0, 7, 0), ("distinct", 5.0, 5, 0), ("sum", 3.0, 4, 1),
            ("avg", 0.75, 4, 1), ("min", -1e16, 4, 1), ("max", 1e16, 4, 1), ("unknown", 0.0, 4, 1),
        ] {
            let result = chart(&session, serde_json::json!({"chart":"time","metric":metric,"field":"source","unit":"number"})).unwrap();
            assert_eq!(result.series[0].points, vec![point], "{metric}");
            assert_eq!(result.series[0].samples, vec![samples], "{metric}");
            assert_eq!(result.incompatible_units, warnings, "{metric}");
            if metric != "count" {
                let absent = chart(&session, serde_json::json!({"chart":"time","metric":metric,"unit":"number"})).unwrap();
                assert_eq!(absent.series[0].points, vec![0.0]);
                assert_eq!(absent.series[0].samples, vec![0]);
                assert_eq!(absent.incompatible_units, 0);
            }
        }
        let unrestricted = chart(&session, serde_json::json!({"chart":"time","metric":"sum","field":"source","unit":"unspecified"})).unwrap();
        assert_eq!(unrestricted.series[0].points, vec![2051.0]);
        assert_eq!(unrestricted.series[0].samples, vec![5]);
        assert_eq!(unrestricted.incompatible_units, 0);
        session.conn().unwrap().execute_batch("DELETE FROM ev WHERE id<4").unwrap();
        for metric in ["sum", "avg", "min", "max"] {
            let empty = chart(&session, serde_json::json!({"chart":"time","metric":metric,"field":"source","unit":"number"})).unwrap();
            assert_eq!(empty.series[0].points, vec![0.0]);
            assert_eq!(empty.series[0].samples, vec![0]);
            assert_eq!(empty.incompatible_units, 1);
        }
    }

    #[test]
    fn series_indexed_buckets_keep_long_split_labels_and_sparse_values() {
        let session = session();
        let conn = session.conn().unwrap();
        conn.execute_batch("SET threads=1; SET memory_limit='32MB'; CREATE TABLE ev(id BIGINT, ts BIGINT, level VARCHAR, source VARCHAR)").unwrap();
        let names: Vec<_> = ["a'", "b\\", "c\n", "dé", "e日", "f", "z"].into_iter().map(|prefix| format!("{prefix}{}", "x".repeat(1024))).collect();
        for (index, name) in names.iter().enumerate() {
            conn.execute("INSERT INTO ev VALUES (?,1,?,'1KB'),(?,101,?,'3s')", duckdb::params![index as i64*2,name,index as i64*2+1,name]).unwrap();
        }
        super::super::udf::register(&conn).unwrap();
        drop(conn);
        for metric in ["sum", "avg", "min", "max"] {
            let result = chart(&session, serde_json::json!({"chart":"time","metric":metric,"field":"source","split":"level","interval_ms":10,"unit":"bytes"})).unwrap();
            assert_eq!(result.series.iter().map(|s| &s.name).collect::<Vec<_>>(), names.iter().take(6).collect::<Vec<_>>());
            assert_eq!(result.incompatible_units, 6, "only selected split groups contribute legacy time warnings");
            for series in result.series {
                let mut points = vec![0.0; 11]; points[0] = 1024.0;
                let mut samples = vec![0; 11]; samples[0] = 1;
                assert_eq!(series.points, points);
                assert_eq!(series.samples, samples);
            }
        }
        session.conn().unwrap().execute_batch("UPDATE ev SET level=NULL").unwrap();
        let empty = chart(&session, serde_json::json!({"chart":"time","metric":"count","split":"level","interval_ms":10})).unwrap();
        assert_eq!(empty.series.len(), 1);
        assert_eq!(empty.series[0].name, "eventos");
        assert_eq!(empty.series[0].points, vec![0.0; 11]);
        assert_eq!(empty.series[0].samples, vec![0; 11]);
        assert_eq!(empty.incompatible_units, 0);
    }

    #[test]
    fn empty_catalog_candidates_do_not_materialize_the_event_side() {
        let session = session();
        let directory = tempfile::tempdir().unwrap();
        let conn = session.conn().unwrap();
        conn.execute_batch(&format!(
            "SET threads=1; SET memory_limit='32MB'; SET max_temp_directory_size='1MB'; \
             SET temp_directory={}; \
             CREATE VIEW ev AS SELECT range::BIGINT AS id, \
             (range % 31)::VARCHAR AS source, (range % 7)::VARCHAR AS code FROM range(2000000); \
             CREATE TABLE enr(source VARCHAR, code VARCHAR, name VARCHAR, description VARCHAR, catalog INTEGER);",
            lit(&directory.path().to_string_lossy())
        )).unwrap();
        super::super::udf::register(&conn).unwrap();
        let mut tests = Tests::default();
        tests.free_text("rareneedle");
        let sql = catalog_candidates_sql(&tests.free[0].names_sql);
        let plan: String = conn.query_row(&format!("EXPLAIN {sql}"), [], |r| r.get(1)).unwrap();
        assert!(!plan.contains("DELIM"), "catalog candidates must not retain all events: {plan}");
        drop(conn);
        assert!(read_ids(&session, &sql).unwrap().is_empty());
        session.conn().unwrap().execute_batch(
            "INSERT INTO enr VALUES ('1','2','ordinary','ordinary',0),('*','3','ordinary','ordinary',1)"
        ).unwrap();
        assert!(read_ids(&session, &sql).unwrap().is_empty());
    }

    #[test]
    fn catalog_candidate_branches_preserve_exact_wildcard_and_duplicate_matches() {
        let session = session();
        let conn = session.conn().unwrap();
        conn.execute_batch(
            "CREATE TABLE ev(id BIGINT, source VARCHAR, code VARCHAR); \
             INSERT INTO ev VALUES (0,'api','42'),(1,'auth','42'),(2,'*','42'),(3,'api','43'),(4,'auth','43'); \
             CREATE TABLE enr(source VARCHAR, code VARCHAR, name VARCHAR, description VARCHAR, catalog INTEGER); \
             INSERT INTO enr VALUES ('api','42','Needle exact','',0),('*','42','','needle wildcard',1),\
             ('api','42','needle duplicate','',1),('api','43','ordinary','',0),('auth','43','needle exact','',0);"
        ).unwrap();
        super::super::udf::register(&conn).unwrap();
        let mut tests = Tests::default();
        tests.free_text("needle");
        drop(conn);
        let mut ids = read_ids(&session, &catalog_candidates_sql(&tests.free[0].names_sql)).unwrap();
        ids.sort_unstable();
        assert_eq!(ids, vec![0, 0, 1, 2, 4]);
        ids.dedup();
        assert_eq!(ids, vec![0, 1, 2, 4]);
    }

    #[test]
    fn selection_limit_rolls_back_and_reuses_a_clean_connection() {
        let session = session();
        let result = crate::resources::with_selection_limit(16, || selection_query(&session, "SELECT range::BIGINT AS id FROM range(100)"));
        assert!(result.err().unwrap().contains("LOGINSIGHT_SELECTION_LIMIT_MB"));
        let count: i64 = session.conn().unwrap().query_row("SELECT count(*) FROM information_schema.tables WHERE table_name LIKE 'sel_%'", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
        let valid = selection_query(&session, "SELECT range::BIGINT AS id FROM range(3)").unwrap();
        assert_eq!(valid.accounted_bytes(), 24);
    }

    #[test]
    fn cancelled_selection_is_not_published_and_retry_succeeds() {
        let session = session();
        let chosen = new_selection(&session, false, None);
        let token = crate::operations::token(Some("cancel-selection-write".into())).unwrap();
        chosen.reserve_rows(&session, 1).unwrap();
        let result = crate::operations::run_with_token(token, || selection_write(&session, &chosen, |conn| {
            conn.execute(&format!("INSERT INTO {} VALUES(42)", chosen.name), []).map_err(err)?;
            crate::operations::cancel_id("cancel-selection-write");
            Ok(1)
        }));
        assert!(result.is_err());
        assert_eq!(chosen.rows.load(Ordering::Relaxed), u64::MAX);
        let valid = selection(&session, &[7, 8]).unwrap();
        assert_eq!(valid.accounted_bytes(), 16);
    }

    #[test]
    fn cache_charges_payload_and_never_retains_oversized_selection() {
        let session = session();
        let small = selection(&session, &[1, 2]).unwrap();
        session.cache_selection("small".into(), Arc::clone(&small));
        assert!(Arc::ptr_eq(&session.cached_selection("small").unwrap(), &small));
        let large = new_selection(&session, false, None);
        large.rows.store(crate::resources::selection_cache_bytes() / 8 + 1, Ordering::Relaxed);
        session.cache_selection("large".into(), large);
        assert!(session.cached_selection("large").is_none());
        assert_eq!(session.selection_snapshot()["accountedBytes"], 16);
    }

    #[test]
    fn inserting_custom_case_selection_preserves_other_owner_retention() {
        use crate::case_resources::{Mode, Policy, Preferences};
        if crate::resources::application_selection_cache_bytes() < 12 << 20 { return; }
        let session = session();
        let make_policy = |case: &str, mib| Policy::capture(Some(&crate::analysis_context::Identity {
            case_id: case.into(), analysis_id: uuid::Uuid::new_v4().to_string(), config_revision: 0, visibility_revision: 0,
        }), &Preferences { schema_version: 1, mode: Mode::Custom, work_limit_mib: Some(mib) }).unwrap();
        let a = make_policy("retention-a", 8); let b = make_policy("retention-b", 16);
        let cache = |policy: Arc<Policy>, key: &str, bytes: u64| crate::case_resources::with(policy, || {
            let selection = new_selection(&session, false, None);
            selection.rows.store(bytes / 8, Ordering::Relaxed);
            session.cache_selection(key.into(), selection);
        });
        cache(b, "owner-b", 6 << 20);
        cache(Arc::clone(&a), "owner-a-old", 3 << 20);
        assert!(session.cached_selection("owner-b").is_some(), "A's 8 MiB cache must not be applied to B");
        assert!(session.cached_selection("owner-a-old").is_some());
        cache(a, "owner-a-new", 6 << 20);
        assert!(session.cached_selection("owner-b").is_some());
        assert!(session.cached_selection("owner-a-old").is_none(), "A trims its own older entry first");
        assert!(session.cached_selection("owner-a-new").is_some());
        assert_eq!(session.selection_snapshot()["entries"], 2);
        session.trim_inactive_selections().unwrap();
    }

    #[test]
    fn owner_selection_pressure_does_not_evict_another_case() {
        use crate::case_work_budget::{Limits, OwnerCounter, Pool};
        let session = session();
        let limits = Limits { materialized: 64, retained: 64, live: 64 };
        let root = Pool::new(limits);
        let owner_a = Arc::new(OwnerCounter::default());
        let a = Pool::child(limits, Arc::clone(&root), Arc::clone(&owner_a), 24);
        let b = Pool::child(limits, Arc::clone(&root), Arc::new(OwnerCounter::default()), 32);
        let active_a = a.reserve(24).unwrap();
        let mut cached_b = new_selection(&session, false, None);
        Arc::get_mut(&mut cached_b).unwrap().pool = b;
        cached_b.reserve_rows(&session, 4).unwrap();
        selection_write(&session, &cached_b, |conn| {
            conn.execute(&format!("INSERT INTO {} VALUES(1),(2),(3),(4)", cached_b.name), []).map_err(err)?; Ok(4)
        }).unwrap();
        session.cache_selection("other-case".into(), Arc::clone(&cached_b));
        drop(cached_b);
        let mut refused = new_selection(&session, false, None);
        Arc::get_mut(&mut refused).unwrap().pool = a;
        assert!(refused.reserve_rows(&session, 1).is_err());
        assert!(session.cached_selection("other-case").is_some());
        assert_eq!(root.used(), 56);
        let mut next = new_selection(&session, false, None);
        Arc::get_mut(&mut next).unwrap().pool = Pool::child(limits, Arc::clone(&root), owner_a, 48);
        next.reserve_rows(&session, 2).unwrap();
        assert!(session.cached_selection("other-case").is_none());
        assert_eq!(root.used(), 40);
        drop((next, refused, active_a));
        session.collect_garbage().unwrap();
        assert_eq!(root.used(), 0);
    }

    #[test]
    fn inactive_cached_selections_release_quota_only_after_real_table_drop() {
        use crate::case_work_budget::{Limits, Pool};
        let session = session();
        let pool = Pool::new(Limits { materialized: 16, retained: 16, live: 16 });
        let mut first = new_selection(&session, false, None);
        Arc::get_mut(&mut first).unwrap().pool = Arc::clone(&pool);
        first.reserve_rows(&session, 2).unwrap();
        selection_write(&session, &first, |conn| {
            conn.execute(&format!("INSERT INTO {} VALUES(1),(2)", first.name), []).map_err(err)?; Ok(2)
        }).unwrap();
        let name = first.name.clone();
        session.cache_selection("quota-old".into(), Arc::clone(&first));
        let active = Arc::clone(&first); drop(first);
        session.trim_inactive_selections().unwrap();
        assert_eq!(pool.used(), 16, "an active cached publication keeps its credit");
        drop(active);
        let mut next = new_selection(&session, false, None);
        Arc::get_mut(&mut next).unwrap().pool = Arc::clone(&pool);
        next.reserve_rows(&session, 1).unwrap();
        assert_eq!(pool.used(), 8, "pressure evicts inactive cache before refusing new work");
        assert_eq!(session.conn().unwrap().query_row(&format!("SELECT count(*) FROM information_schema.tables WHERE table_name='{}'", name), [], |row| row.get::<_,i64>(0)).unwrap(), 0);
        drop(next);
        assert_eq!(pool.used(), 8, "queued garbage retains its quota");
        session.collect_garbage().unwrap();
        assert_eq!(pool.used(), 0);
    }

    #[test]
    fn failed_or_cancelled_selection_cleanup_retains_every_pending_table_for_retry() {
        let session = session();
        session.conn().unwrap().execute_batch("CREATE VIEW retired_wrong_type AS SELECT 1; CREATE TABLE retired_later(id BIGINT)").unwrap();
        session.garbage().lock().extend([SelectionGarbage::new("retired_wrong_type"), SelectionGarbage::new("retired_later")]);
        let next = new_selection(&session, false, None);
        let mut filled = false;
        assert!(selection_write(&session, &next, |_| { filled = true; Ok(0) }).is_err());
        assert!(!filled, "failed cleanup cannot begin publishing another selection");
        assert_eq!(session.garbage().lock().iter().map(|item| item.name.as_str()).collect::<Vec<_>>(), vec!["retired_wrong_type", "retired_later"]);
        session.conn().unwrap().execute_batch("DROP VIEW retired_wrong_type").unwrap();
        let token = crate::operations::token(Some("selection-cleanup-cancel".into())).unwrap();
        assert!(crate::operations::run_with_token(token, || {
            crate::operations::cancel_id("selection-cleanup-cancel");
            session.collect_garbage()
        }).is_err());
        assert_eq!(session.garbage().lock().iter().map(|item| item.name.as_str()).collect::<Vec<_>>(), vec!["retired_wrong_type", "retired_later"]);
        session.collect_garbage().unwrap();
        assert!(session.garbage().lock().is_empty());
        assert_eq!(session.conn().unwrap().query_row("SELECT count(*) FROM information_schema.tables WHERE table_name='retired_later'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        next.reserve_rows(&session, 1).unwrap();
        selection_write(&session, &next, |conn| { conn.execute(&format!("INSERT INTO {} VALUES(7)", next.name), []).map_err(err)?; Ok(1) }).unwrap();
        assert_eq!(next.accounted_bytes(), 8);
    }

    #[test]
    fn waiting_for_same_selection_is_cancellable_without_poisoning_builder() {
        let session = Arc::new(session());
        let held = session.begin_selection("filter").unwrap();
        let token = crate::operations::token(Some("selection-waiter".into())).unwrap();
        let other = Arc::clone(&session);
        let (send, receive) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || crate::operations::run_with_token(token, || {
            send.send(()).unwrap();
            other.begin_selection("filter").map(|_| ())
        }));
        receive.recv().unwrap();
        crate::operations::cancel_id("selection-waiter");
        assert!(waiter.join().unwrap().is_err());
        drop(held);
        assert!(session.begin_selection("filter").is_ok());
    }

    #[test]
    fn streaming_fetch_propagates_late_sql_failure() {
        let session = session();
        let mut seen = 0;
        let error = stream_rows(&session, "SELECT CASE WHEN range >= 4096 THEN error('late fetch') ELSE range END::BIGINT FROM range(10000)", |row| {
            let _: i64 = row.get(0)?; seen += 1; Ok(())
        }).unwrap_err();
        assert!(error.contains("late fetch"));
        // Execution can fail before the first chunk on some DuckDB versions;
        // neither timing is permitted to turn a partial stream into success.
        assert!(seen < 10000);
        assert_eq!(session.conn().unwrap().query_row("SELECT 42", [], |r| r.get::<_, i64>(0)).unwrap(), 42);
    }

    #[test]
    fn sql_spill_budget_failure_is_returned_and_connection_remains_usable() {
        let session = session();
        let temp = tempfile::tempdir().unwrap();
        session.conn().unwrap().execute_batch(&format!(
            "SET temp_directory={}; SET memory_limit='4MB'; SET max_temp_directory_size='1B'; SET threads=1;",
            lit(&temp.path().to_string_lossy()),
        )).unwrap();
        let error = stream_rows(&session, "SELECT repeat('x', 1024) || range::VARCHAR AS v FROM range(30000) ORDER BY v DESC", |_| Ok(())).unwrap_err();
        assert!(error.contains("Memory") || error.contains("memory") || error.contains("temporary"), "{error}");
        assert_eq!(session.conn().unwrap().query_row("SELECT 42", [], |r| r.get::<_, i64>(0)).unwrap(), 42);
    }

    #[test]
    fn plain_required_filters_are_cacheable_but_mutable_rules_are_not() {
        let filters = |op: &str, value: &str| prepared(&[crate::query::Filter { column: "_all".into(), op: op.into(), value: value.into(), value2: None }]);
        assert!(cacheable_filters(&filters("query", "timeout OR user:root")));
        assert!(!cacheable_filters(&filters("threat_rule", "whatever")));
        assert!(!cacheable_filters(&filters("detection", "whatever")));
    }

    #[test]
    fn complete_filter_cache_precedes_eight_term_lru_churn_and_cancel_checks() {
        struct Restore(Option<std::ffi::OsString>, bool);
        impl Drop for Restore {
            fn drop(&mut self) {
                match &self.0 { Some(value) => std::env::set_var("LOGINSIGHT_ENGINE_DIR", value), None => std::env::remove_var("LOGINSIGHT_ENGINE_DIR") }
                crate::engine::set_enabled(self.1);
            }
        }
        let _restore = Restore(std::env::var_os("LOGINSIGHT_ENGINE_DIR"), super::super::enabled());
        let directory = tempfile::tempdir().unwrap();
        std::env::set_var("LOGINSIGHT_ENGINE_DIR", directory.path().join("engine"));
        crate::engine::set_enabled(true);
        let words = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel"];
        let path = directory.path().join("cache-order.jsonl");
        let records: Vec<_> = (0..80).map(|id| serde_json::json!({
            "timestamp":"2026-01-01T00:00:00Z", "message":words.join(" "), "keep":id % 2 == 0,
            "request_id":format!("{id:032x}"),
        }).to_string()).collect();
        std::fs::write(&path, records.join("\n")).unwrap();
        let index = crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let config = CodesConfig::default();
        crate::engine::prepare(&index, &config, &config, &[], &|_, _| {}).unwrap();
        let source = Source { idx: &index, codes: &config, system: &config, derived: &[] };
        let session = super::super::session_checked(&index, &config, &config, &[]).unwrap().unwrap();
        let mut filters: Vec<_> = words.iter().map(|word| crate::query::Filter {
            column:"_all".into(), op:"query".into(), value:(*word).into(), value2:None,
        }).collect();
        filters.push(crate::query::Filter { column:"raw".into(), op:"contains".into(), value:"\"keep\":true".into(), value2:None });
        let pfs = prepared(&filters);
        assert_eq!(session.schema.plan(&pfs).tests.free.len(), 8, "all eight required words have real text probes");
        assert!(!session.schema.plan(&pfs).exact(), "the complete result must include canonical residual verification");
        FREE_CANDIDATE_PROBES.with(|probes| probes.set(0));
        assert_eq!(count_session(&session, &source, &pfs).unwrap(), 40);
        assert_eq!(FREE_CANDIDATE_PROBES.with(std::cell::Cell::get), 8);
        let before = session.selection_snapshot();
        assert_eq!(before["entries"], 8, "whole-filter and term selections share the bounded LRU");
        FREE_CANDIDATE_PROBES.with(|probes| probes.set(0));
        assert_eq!(count_session(&session, &source, &pfs).unwrap(), 40);
        assert_eq!(FREE_CANDIDATE_PROBES.with(std::cell::Cell::get), 0, "a completed filter must skip all repeated term probes");
        assert_eq!(session.selection_snapshot(), before, "the retained selection must not be evicted by term churn");
        for direction in ["asc", "desc"] {
            let page = select_page(&source, &pfs, "id", direction, 0, 3, None).unwrap().unwrap();
            let expected = if direction == "asc" { vec![0, 2, 4] } else { vec![78, 76, 74] };
            assert_eq!(page.ids, expected);
            assert!(page.has_more);
            let next = select_page(&source, &pfs, "id", direction, 3, 3, page.next_cursor.as_deref()).unwrap().unwrap();
            assert_eq!(next.ids, if direction == "asc" { vec![6, 8, 10] } else { vec![72, 70, 68] });
            assert_eq!(FREE_CANDIDATE_PROBES.with(std::cell::Cell::get), 0, "page hits must skip term probing too");
            assert_eq!(session.selection_snapshot(), before);
        }
        let token = crate::operations::token(Some("complete-filter-hit-cancel".into())).unwrap();
        let cancelled = crate::operations::run_with_token(token, || {
            crate::operations::cancel_id("complete-filter-hit-cancel");
            count_session(&session, &source, &pfs)
        });
        assert!(cancelled.is_err());
        assert_eq!(FREE_CANDIDATE_PROBES.with(std::cell::Cell::get), 0);
        assert_eq!(session.selection_snapshot(), before);
        let hex = prepared(&[
            crate::query::Filter { column:"request_id".into(), op:"equals_exact".into(), value:format!("{:032x}", 4), value2:None },
            crate::query::Filter { column:"raw".into(), op:"contains".into(), value:"\"keep\":true".into(), value2:None },
        ]);
        assert_eq!(count_session(&session, &source, &hex).unwrap(), 1);
        // Retain the full result in the eighth slot while evicting its term.
        // The old first-page shortcut would re-probe and evict that result.
        for i in 0..7 { session.cache_selection(format!("hex-churn-{i}"), selection(&session, &[1]).unwrap()); }
        EXACT_HEX_CANDIDATE_PROBES.with(|probes| probes.set(0));
        let page = select_page(&source, &hex, "id", "asc", 0, 3, None).unwrap().unwrap();
        assert_eq!(page.ids, vec![4]);
        assert_eq!(EXACT_HEX_CANDIDATE_PROBES.with(std::cell::Cell::get), 0, "complete hits precede even the first-page singleton probe");
        let key = complete_selection_key(&session, &pfs).unwrap();
        session.names_version.fetch_add(1, Ordering::SeqCst);
        assert_ne!(complete_selection_key(&session, &pfs).unwrap(), key, "catalog versions cannot reuse an earlier complete result");
        filters[0].value = "changed".into();
        assert_ne!(complete_selection_key(&session, &prepared(&filters)).unwrap(), complete_selection_key(&session, &pfs).unwrap());
    }

    fn scope_fixture() -> (tempfile::TempDir, FileIndex) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("page-scope.jsonl");
        std::fs::write(&path, "{\"message\":\"keep\"}\n{\"message\":\"drop\"}\n{\"message\":\"keep\"}\n").unwrap();
        let index = crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        (directory, index)
    }

    fn filter(column: &str, op: &str, value: &str) -> crate::query::Filter {
        crate::query::Filter { column: column.into(), op: op.into(), value: value.into(), value2: None }
    }

    #[test]
    fn page_scope_miss_never_builds_or_waits_for_complete_selection() {
        let (_directory, index) = scope_fixture();
        let config = CodesConfig::default();
        let source = Source { idx: &index, codes: &config, system: &config, derived: &[] };
        let session = session();
        let pfs = prepared(&[filter("id", "gte", "0"), filter("raw", "contains", "keep")]);
        let key = complete_selection_key(&session, &pfs).unwrap();
        let _building = session.begin_selection(&key).unwrap();
        let token = crate::operations::token(Some("page-hit-only-miss".into())).unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        // A regression that joins the analytic builder fails after a bounded
        // cancellation instead of hanging the entire test process.
        let cancel = std::thread::spawn(move || {
            if receive.recv_timeout(std::time::Duration::from_secs(2)).is_err() {
                crate::operations::cancel_id("page-hit-only-miss");
            }
        });
        let result = crate::operations::run_with_token(token, || page_scope(&session, &source, &pfs, false));
        let _ = send.send(());
        cancel.join().unwrap();
        let PageScope::Planned { scope, verify, .. } = result.unwrap().unwrap() else { panic!("not a singleton") };
        assert!(scope._selection.is_none());
        assert_eq!(verify, vec![1], "SQL-proven id predicate must not be repeated");
        assert_eq!(session.selection_snapshot()["entries"], 0);
        assert_eq!(session.selection_builds.lock().len(), 1, "only the preexisting builder may exist");
    }

    #[test]
    fn page_scope_exact_predicates_stay_direct_after_complete_analytics_cache() {
        let (_directory, index) = scope_fixture();
        let config = CodesConfig::default();
        let source = Source { idx: &index, codes: &config, system: &config, derived: &[] };
        let session = session();
        {
            let conn = session.conn().unwrap();
            super::super::udf::register(&conn).unwrap();
            conn.execute_batch("CREATE TABLE ev(id BIGINT, message VARCHAR); INSERT INTO ev VALUES (0,'keep'),(1,'drop'),(2,'keep')").unwrap();
        }
        for op in ["contains", "equals_exact"] {
            let pfs = prepared(&[filter("message", op, "keep")]);
            let plan = session.schema.plan(&pfs);
            assert!(plan.exact());
            assert!(costly(&plan.sql), "analytics may retain this complete selection");
            let key = complete_selection_key_for_plan(&session, &pfs, &plan).unwrap();
            let PageScope::Planned { scope, verify } = page_scope(&session, &source, &pfs, false).unwrap() else { panic!("not a singleton") };
            assert!(scope._selection.is_none());
            assert!(verify.is_empty());
            let cold = read_ids(&session, &format!("SELECT id FROM ev WHERE {} ORDER BY id", scope.cond)).unwrap();
            assert_eq!(cold, vec![0, 2]);
            drop(scope);
            assert_eq!(count_session(&session, &source, &pfs).unwrap(), 2);
            let completed = session.cached_selection(&key).expect("analytics retained the complete result");
            let before = session.selection_snapshot();
            for first_page in [true, false] {
                let PageScope::Planned { scope, verify } = page_scope(&session, &source, &pfs, first_page).unwrap() else { panic!("not a singleton") };
                assert!(scope._selection.is_none(), "SQL-exact paging must retain its direct predicate after count");
                assert!(scope._free.is_empty());
                assert!(verify.is_empty());
                    assert_eq!(read_ids(&session, &format!("SELECT id FROM ev WHERE {} ORDER BY id", scope.cond)).unwrap(), cold);
            }
            assert!(session.selection_builds.lock().is_empty());
            assert_eq!(session.selection_snapshot(), before);
            assert!(Arc::ptr_eq(&session.cached_selection(&key).unwrap(), &completed));
        }
    }

    #[test]
    fn page_scope_hit_keeps_selection_lease_across_eviction_and_checks_cancellation() {
        let (_directory, index) = scope_fixture();
        let config = CodesConfig::default();
        let source = Source { idx: &index, codes: &config, system: &config, derived: &[] };
        let session = session();
        session.conn().unwrap().execute_batch("CREATE TABLE ev(id BIGINT); INSERT INTO ev VALUES (0),(1),(2)").unwrap();
        let pfs = prepared(&[filter("raw", "contains", "keep")]);
        let key = complete_selection_key(&session, &pfs).unwrap();
        let chosen = selection(&session, &[0, 2]).unwrap();
        let table = chosen.name.clone();
        session.cache_selection(key.clone(), chosen);
        let before = session.selection_snapshot();
        let token = crate::operations::token(Some("page-hit-only-cancel".into())).unwrap();
        assert!(crate::operations::run_with_token(token, || {
            crate::operations::cancel_id("page-hit-only-cancel");
            assert!(page_scope(&session, &source, &pfs, false).is_err());
        }).is_err());
        assert_eq!(session.selection_snapshot(), before);
        let PageScope::Planned { scope, verify, .. } = page_scope(&session, &source, &pfs, false).unwrap() else { panic!("not a singleton") };
        assert!(verify.is_empty());
        assert!(scope._selection.is_some());
        for i in 0..8 { session.cache_selection(format!("evict-{i}"), selection(&session, &[1]).unwrap()); }
        assert!(session.cached_selection(&key).is_none());
        session.collect_garbage().unwrap();
        assert_eq!(read_ids(&session, &format!("SELECT id FROM ev WHERE {} ORDER BY id", scope.cond)).unwrap(), vec![0, 2]);
        drop(scope);
        session.collect_garbage().unwrap();
        let remaining: i64 = session.conn().unwrap().query_row(
            "SELECT count(*) FROM information_schema.tables WHERE table_name=?", [&table], |row| row.get(0),
        ).unwrap();
        assert_eq!(remaining, 0, "only the last lease may retire a selection table");
    }

    #[test]
    fn page_scope_keys_reject_catalog_filter_and_mutable_rule_changes() {
        let (_directory, index) = scope_fixture();
        let config = CodesConfig::default();
        let source = Source { idx: &index, codes: &config, system: &config, derived: &[] };
        let session = session();
        let pfs = prepared(&[filter("raw", "contains", "keep")]);
        let key = complete_selection_key(&session, &pfs).unwrap();
        session.cache_selection(key, selection(&session, &[0, 2]).unwrap());
        let changed = prepared(&[filter("raw", "contains", "drop")]);
        for pfs in [&changed, &prepared(&[filter("_all", "detection", "missing")]), &prepared(&[filter("_all", "threat_rule", "missing")])] {
            let PageScope::Planned { scope, verify, .. } = page_scope(&session, &source, pfs, false).unwrap() else { panic!("not a singleton") };
            assert!(scope._selection.is_none());
            assert_eq!(verify, vec![0]);
        }
        session.names_version.fetch_add(1, Ordering::SeqCst);
        let PageScope::Planned { scope, verify, .. } = page_scope(&session, &source, &pfs, false).unwrap() else { panic!("not a singleton") };
        assert!(scope._selection.is_none());
        assert_eq!(verify, vec![0]);
    }

    #[test]
    fn page_line_residuals_bypass_analytic_cache_and_preserve_literal_spaces() {
        let (_directory, index) = scope_fixture();
        let config = CodesConfig::default();
        let source = Source { idx: &index, codes: &config, system: &config, derived: &[] };
        let session = session();
        for op in ["contains", "not_contains", "regex"] {
            let pfs = prepared(&[filter("_all", op, " keep ")]);
            assert_eq!(session.schema.plan(&pfs).lines, vec![0]);
            // This entry must never be trusted as canonical page membership.
            session.cache_selection(complete_selection_key(&session, &pfs).unwrap(), selection(&session, &[0, 2]).unwrap());
            let PageScope::Planned { scope, verify, .. } = page_scope(&session, &source, &pfs, false).unwrap() else { panic!("not a singleton") };
            assert!(scope._selection.is_none());
            assert_eq!(verify, vec![0]);
            let verifier = crate::query::CandidateVerifier::new(&index, &pfs, &[], &verify, &config, &config, &[]).unwrap();
            for id in 0..index.lines.len() {
                assert_eq!(verifier.matches(id), crate::query::matches_indexed(&source.event(id), &pfs[0]), "{op} row {id}");
            }
        }
    }

    fn bare_scope(session: &Session) -> Scope<'_> {
        Scope { session, cond: "TRUE".into(), names: false, _tests: Tests::default(), _selection: None, _free: Vec::new() }
    }

    fn latency_value(sample: crate::insights::LatencySample) -> Value {
        let empty = crate::insights::overview(std::iter::empty::<Event>);
        let result = crate::insights::finish_overview(empty, Vec::new(), HashMap::new(), sample, |_, _, _, _| {});
        serde_json::to_value(result.latency).unwrap()
    }

    #[test]
    fn latency_stream_preserves_selected_field_units_and_exact_reservoir_order() {
        let mut session = session();
        session.schema.fields.insert("latency_ms".into(), "latency_ms".into());
        session.schema.fields.insert("duration_ms".into(), "duration_ms".into());
        let conn = session.conn().unwrap();
        super::super::udf::register(&conn).unwrap();
        conn.execute_batch("SET threads=1; CREATE VIEW ev AS SELECT range::BIGINT AS id, \
            CASE WHEN range=0 THEN NULL ELSE '9999' END AS latency_ms, \
            CASE WHEN range=0 THEN '20' WHEN range%97=0 THEN NULL WHEN range%19=0 THEN 'invalid' \
            WHEN range%7=0 THEN '2s' ELSE (range%200)::VARCHAR END AS duration_ms FROM range(12017)").unwrap();
        drop(conn);
        let mut expected = crate::insights::LatencySample::default();
        for id in 0..12017 {
            let duration = if id == 0 { Some("20".to_string()) } else if id % 97 == 0 { None }
                else if id % 19 == 0 { Some("invalid".into()) } else if id % 7 == 0 { Some("2s".into()) }
                else { Some((id % 200).to_string()) };
            expected.push(|key| match key { "latency_ms" if id != 0 => Some("9999".into()), "duration_ms" => duration.clone(), _ => None });
        }
        let actual = latency_value(latency_of(&bare_scope(&session), "ev").unwrap());
        assert_eq!(actual, latency_value(expected));
        assert_eq!(actual["field"], "duration_ms");
        assert_eq!(actual["sampled"], 10000);
        assert!(actual["count"].as_u64().unwrap() > 10000);
    }

    #[test]
    fn light_sql_stream_preserves_requested_values_and_pivot_results() {
        let mut session = session();
        session.schema.fields.insert("extra".into(), "extra".into());
        session.conn().unwrap().execute_batch("SET threads=1; CREATE VIEW ev AS SELECT range::BIGINT AS id, \
            CASE WHEN range%3=0 THEN NULL ELSE (range-2)*1000 END::BIGINT AS ts, \
            CASE WHEN range%2=0 THEN NULL ELSE 'api' END AS source, \
            'Mensagem '||range::VARCHAR AS message, \
            CASE WHEN range%3=0 THEN NULL ELSE repeat('ação',128)||range::VARCHAR END AS extra \
            FROM range(33) ORDER BY range DESC").unwrap();
        let expected: Vec<Event> = (0..33).map(|id| {
            let mut event = Event::empty(); event.id = id;
            event.timestamp = (id % 3 != 0).then_some((id as i64 - 2) * 1000);
            event.source = if id % 2 == 0 { "" } else { "api" }.into();
            event.message = format!("Mensagem {id}");
            if id % 3 != 0 { event.fields.insert("extra".into(), Value::String(format!("{}{id}", "ação".repeat(128)))); }
            event
        }).collect();
        let columns = ["id", "timestamp", "source", "message", "extra"].map(str::to_string);
        let work = LightStreamWork::default();
        let actual = light_events_bounded(&bare_scope(&session), &columns, 8, 2048, &work, |events| events.collect::<Vec<_>>()).unwrap();
        assert_eq!(serde_json::to_value(actual).unwrap(), serde_json::to_value(&expected).unwrap());
        assert_eq!(work.rows.load(Ordering::Relaxed), 33);
        assert!(work.max_batch_rows.load(Ordering::Relaxed) <= 8);
        assert!(work.max_batch_bytes.load(Ordering::Relaxed) <= 2048);
        let spec: crate::analysis::PivotSpec = serde_json::from_value(serde_json::json!({
            "rows":["source"],"cols":["extra"],"values":[{"column":"message","func":"count","alias":"n"}],"limit_rows":10
        })).unwrap();
        let actual = light_events(&bare_scope(&session), &columns, |events| crate::analysis::pivot_stream(events, &spec)).unwrap().unwrap();
        let expected = crate::analysis::pivot(&expected, &spec).unwrap();
        assert_eq!(serde_json::to_value(actual).unwrap(), serde_json::to_value(expected).unwrap());
    }

    #[test]
    fn light_sql_and_latency_fetch_failures_do_not_publish_partial_results() {
        let mut session = session();
        session.schema.fields.insert("duration_ms".into(), "message".into());
        let conn = session.conn().unwrap();
        super::super::udf::register(&conn).unwrap();
        conn.execute_batch("SET threads=1; CREATE VIEW ev AS SELECT range::BIGINT AS id, range::BIGINT AS ts, \
            CASE WHEN range>=4096 THEN error('late light value') ELSE range::VARCHAR END AS message FROM range(5000)").unwrap();
        drop(conn);
        let scope = bare_scope(&session);
        let error = light_events(&scope, &["message".into()], |events| events.count()).unwrap_err();
        assert!(error.contains("late light value"), "{error}");
        let error = latency_of(&scope, "ev").err().unwrap();
        assert!(error.contains("late light value"), "{error}");
        assert_eq!(session.conn().unwrap().query_row("SELECT 42", [], |row| row.get::<_, i64>(0)).unwrap(), 42);
    }
}

// ---------------------------------------------------------------- matches

/// `query::indexed_matches` for prepared filters.
pub(crate) fn matches(src: &Source, pfs: &[PreparedFilter]) -> Result<Option<Vec<usize>>> {
    analytics_with(src, false, |session| matching_ids(session, src, pfs))
}

pub(crate) fn visit_matches(src: &Source, pfs: &[PreparedFilter], visit: impl FnMut(usize) -> Result<bool>) -> Result<Option<()>> {
    analytics_with(src, false, |session| {
        let scope = scope(session, src, pfs)?;
        // Ordered callbacks preserve first-N/export/text accumulation semantics;
        // DuckDB's buffer/spill budgets cover any required sort.
        visit_ids(session, &format!("SELECT id FROM {} WHERE {} ORDER BY id", scope.from(false), scope.cond), visit)
    })
}

/// Stream exactly matching global row IDs without materializing a selection
/// table or formatting Events. None means unavailable/residual capability, never
/// an execution/cancellation/visibility failure.
pub(crate) fn visit_exact_matches(src: &Source, pfs: &[PreparedFilter], visit: impl FnMut(usize) -> Result<bool>) -> Result<Option<()>> {
    Ok(analytics_with(src, base_page_safe(pfs, ""), |session| {
        if !session.schema.plan(pfs).exact() { return Ok(None); }
        let PageScope::Planned { scope, verify, .. } = page_scope(session, src, pfs, false)? else { return Ok(None); };
        if !verify.is_empty() { return Ok(None); }
        visit_ids(session, &format!("SELECT id FROM {} WHERE {} ORDER BY id", scope.from(false), scope.cond), visit)?;
        Ok(Some(()))
    })?.flatten())
}

fn count_session(session: &Session, src: &Source, pfs: &[PreparedFilter]) -> Result<usize> {
    if pfs.is_empty() {
        crate::operations::check()?;
        // The admitted gate already counts exact visible positional rows. Its
        // accessor validates source/payload leases before this scan-free return.
        if let Some(gate) = crate::analysis_runtime::indexed_gate(src.idx)? {
            return Ok(gate.visible_count());
        }
    }
    if crate::analysis_runtime::visibility_unrestricted(src.idx)? {
    if let (Some(predicate), Some(readers)) = (super::time_index::predicate(pfs), session.exact_time_indexes()) {
        crate::operations::progress("analytics-time-index", "Lendo resumo temporal verificado", 0, 0, 0);
        if let Some(total) = super::time_index::count(&readers, &predicate)? { return Ok(total); }
        session.invalidate_exact_times(&readers)?;
    }
    }
    let scope = scope(session, src, pfs)?;
    let counted = rows(
        session,
        &format!("SELECT count(*) FROM {} WHERE {}", scope.from(false), scope.cond),
        |r| r.get::<_, i64>(0),
    )?;
    Ok(counted.first().copied().unwrap_or(0) as usize)
}

pub(crate) fn count(src: &Source, filters: &[crate::query::Filter]) -> Result<Option<usize>> {
    let pfs = prepared(filters);
    analytics_with(src, base_page_safe(&pfs, ""), |session| count_session(session, src, &pfs))
}

// ---------------------------------------------------------------- stats

const LEVEL_LABEL: &str = "CASE lvl WHEN 1 THEN 'Crítico' WHEN 2 THEN 'Erro' WHEN 3 THEN 'Aviso' \
     WHEN 4 THEN 'Depuração' WHEN 5 THEN 'Rastreio' ELSE 'Informação' END";

fn stats_of(scope: &Scope) -> Result<Stats> {
    let from = scope.from(false);
    let cond = &scope.cond;
    // Histogram boundaries depend on the selected timestamp range. Obtain
    // that range and the level counts together, so the exact histogram needs
    // only one further pass instead of scanning for levels separately.
    // lvl is a stored UTINYINT; this aggregation has at most 256 groups.
    let summaries = rows(
        scope.session,
        &format!("SELECT lvl, count(*), min(NULLIF(ts, 0)), max(NULLIF(ts, 0)) FROM {from} WHERE {cond} GROUP BY lvl"),
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, Option<i64>>(3)?)),
    )?;
    let mut merged: HashMap<&'static str, i64> = HashMap::new();
    let (mut min_ts, mut max_ts): (Option<i64>, Option<i64>) = (None, None);
    for (lvl, count, lower, upper) in summaries {
        *merged.entry(class_label(lvl as u8)).or_default() += count;
        if let Some(lower) = lower { min_ts = Some(min_ts.map_or(lower, |min| min.min(lower))); }
        if let Some(upper) = upper { max_ts = Some(max_ts.map_or(upper, |max| max.max(upper))); }
    }
    let mut buckets = Vec::new();
    let mut bucket_ms = 0;
    if let (Some(min_ts), Some(max_ts)) = (min_ts, max_ts) {
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
    let mut levels: Vec<(String, i64)> = merged.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    sort_levels(&mut levels);
    Ok(build_stats(buckets, bucket_ms, levels))
}

fn stats_session(session: &Session, src: &Source, pfs: &[PreparedFilter]) -> Result<Stats> {
    if crate::analysis_runtime::visibility_unrestricted(src.idx)? {
    if let (Some(predicate), Some(readers)) = (super::time_index::predicate(pfs), session.exact_time_indexes()) {
        crate::operations::progress("analytics-time-index", "Lendo resumo temporal verificado", 0, 0, 0);
        if let Some(stats) = super::time_index::stats(&readers, &predicate)? { return Ok(stats); }
        session.invalidate_exact_times(&readers)?;
    }
    }
    stats_of(&scope(session, src, pfs)?)
}

pub(crate) fn stats(src: &Source, pfs: &[PreparedFilter]) -> Result<Option<Stats>> {
    analytics_with(src, base_page_safe(pfs, ""), |session| stats_session(session, src, pfs))
}

fn timeline_session(session: &Session, start: i64, end: i64, width: i64, buckets: usize) -> Result<Option<super::time_index::Histogram>> {
    let Some(readers) = session.exact_time_indexes() else { return Ok(None); };
    crate::operations::progress("analytics-time-index", "Lendo resumo temporal verificado", 0, 0, 0);
    let result = super::time_index::histogram(&readers, start, end, width, buckets)?;
    if result.is_none() { session.invalidate_exact_times(&readers)?; }
    Ok(result)
}

fn sparse_timeline_session(
    session: &Session, src: &Source, mask: &crate::analysis_visibility::Mask,
    plan: super::sparse_timeline::Plan, start: i64, end: i64, width: i64, buckets: usize,
) -> Result<Option<super::time_index::Histogram>> {
    let readers = session.exact_time_indexes();
    let result = super::sparse_timeline::histogram(plan, &src.idx.lines, mask, readers.as_ref(), start, end, width, buckets)?;
    if result.is_none() {
        if let Some(readers) = &readers { session.invalidate_exact_times(readers)?; }
    }
    Ok(result.map(|(histogram, _work)| histogram))
}

/// The caller has already bound this mask to the admitted indexed source.
/// A visible remainder needs only those retained metadata/source/payload leases;
/// waiting for a Session here would otherwise trigger an unnecessary full scan.
fn sparse_timeline_query(
    src: &Source, mask: &crate::analysis_visibility::Mask,
    plan: super::sparse_timeline::Plan, start: i64, end: i64, width: i64, buckets: usize,
) -> Result<Option<super::time_index::Histogram>> {
    if !plan.requires_base() {
        return Ok(super::sparse_timeline::histogram(
            plan, &src.idx.lines, mask, None, start, end, width, buckets,
        )?.map(|(histogram, _work)| histogram));
    }
    Ok(analytics_with(src, true, |session|
        sparse_timeline_session(session, src, mask, plan, start, end, width, buckets)
    )?.flatten())
}

/// Optional exact acceleration for the unfiltered indexed timeline. Callers
/// retain their existing source scan when no complete verified capability exists.
pub(crate) fn timeline_histogram(src: &Source, start: i64, end: i64, width: i64, buckets: usize) -> Result<Option<super::time_index::Histogram>> {
    if let Some(gate) = crate::analysis_runtime::indexed_gate(src.idx)? {
        if !gate.is_unrestricted() {
            if src.idx.lines.len() < super::sparse_timeline::MIN_SOURCE_ROWS { return Ok(None); }
            let Some(mask) = gate.indexed_mask() else { return Ok(None); };
            let Some(plan) = super::sparse_timeline::Plan::new(src.idx, mask)? else { return Ok(None); };
            return sparse_timeline_query(src, mask, plan, start, end, width, buckets);
        }
    }
    Ok(analytics_with(src, true, |session| timeline_session(session, start, end, width, buckets))?.flatten())
}

#[cfg(test)]
mod exact_time_routing_tests {
    use super::*;
    use super::super::time_index;
    use parking_lot::{Mutex, RwLock};
    use serde_json::json;
    use std::path::PathBuf;

    fn fixture() -> (tempfile::TempDir, Session, Vec<PathBuf>, time_index::ReadSet) {
        let dir = tempfile::tempdir().unwrap();
        let connection = duckdb::Connection::open_in_memory().unwrap();
        connection.execute_batch("SET threads=1; SET memory_limit='32MB'; SET max_temp_directory_size='1MB'; \
            CREATE TABLE ev(id BIGINT, lvl UTINYINT, ts BIGINT, level VARCHAR, source VARCHAR)").unwrap();
        let mut handles = Vec::new();
        let mut paths = Vec::new();
        let parts = [vec![(0, Some(5)), (1, Some(-100)), (2, None), (3, Some(0))],
                     vec![(4, Some(5)), (5, Some(101)), (6, Some(-1)), (2, Some(101)), (0, None)]];
        let mut id = 0i64;
        for (part, rows) in parts.iter().enumerate() {
            let store = dir.path().join(format!("part{part}.duckdb"));
            let producer = duckdb::Connection::open_in_memory().unwrap();
            producer.execute_batch("SET threads=1; SET memory_limit='32MB'; SET max_temp_directory_size='1MB'; CREATE TABLE ev(lvl UTINYINT, ts BIGINT)").unwrap();
            for &(level, timestamp) in rows {
                producer.execute("INSERT INTO ev VALUES (?, ?)", duckdb::params![level, timestamp]).unwrap();
                let label = if level == 6 { "custom" } else { class_label(level) };
                connection.execute("INSERT INTO ev VALUES (?, ?, ?, ?, ?)", duckdb::params![id, level, timestamp, label, if part == 0 { "api" } else { "auth" }]).unwrap();
                id += 1;
            }
            std::fs::write(store.with_extension("complete.json"), json!({"key":format!("part{part}"), "rows":rows.len(), "version":5}).to_string()).unwrap();
            let identity = time_index::identity(&store, rows.len()).unwrap();
            time_index::ensure(&producer, &store, &identity, &|| false).unwrap();
            handles.push(time_index::open(&store, &identity).unwrap().unwrap());
            paths.push(store);
        }
        let session = Session {
            row_count: None,
            key: "time-routing-test".into(), base: Mutex::new(connection), pool: Mutex::new(Vec::new()),
            schema: super::super::sql::Schema::default(), timestamps_non_null: false, baked: true,
            names: RwLock::new(None), names_version: AtomicU64::new(0), selections: Mutex::new(Vec::new()),
            selection_builds: Mutex::new(std::collections::HashSet::new()), selection_changed: parking_lot::Condvar::new(),
            garbage: Arc::new(Mutex::new(Vec::new())), texts: Vec::new(), time_indexes: RwLock::new(None), _leases: Vec::new(),
        };
        (dir, session, paths, handles.into())
    }
    fn index() -> FileIndex {
        FileIndex { parts: Vec::new(), lines: Arc::new(crate::metadata_store::LineStore::default()), columns: Vec::new(), time_order: std::sync::Arc::new(std::sync::OnceLock::new()) }
    }
    fn filters(value: Value) -> Vec<PreparedFilter> {
        prepared(&serde_json::from_value::<Vec<crate::query::Filter>>(value).unwrap())
    }

    #[test]
    fn admitted_mask_answers_empty_count_without_sql_but_rejects_corrupted_membership() {
        use crate::{
            analysis_context::Snapshot,
            analysis_runtime::{self, Mode},
            exclusion_store::{self, Admission, Purpose, Work},
            source_publication, AppState, SourceData,
        };
        struct Environment(Option<std::ffi::OsString>);
        impl Drop for Environment {
            fn drop(&mut self) {
                match &self.0 {
                    Some(value) => std::env::set_var("LOGINSIGHT_DATA_DIR", value),
                    None => std::env::remove_var("LOGINSIGHT_DATA_DIR"),
                }
            }
        }
        let (directory, session, _, _) = fixture();
        let _environment = Environment(std::env::var_os("LOGINSIGHT_DATA_DIR"));
        std::env::set_var("LOGINSIGHT_DATA_DIR", directory.path());
        crate::case_store::save_at(directory.path(), json!({"cases":[{"id":"a"}]})).unwrap();
        let initial: Snapshot = serde_json::from_value(
            crate::case_store::load_at(directory.path()).unwrap()["cases"][0]["analysisContext"]
                .clone(),
        )
        .unwrap();
        let raw = directory.path().join("source.jsonl");
        std::fs::write(
            &raw,
            "{\"message\":\"one\",\"timestamp\":\"1970-01-01T00:00:10Z\",\"level\":\"ERROR\"}\n{\"message\":\"two\",\"timestamp\":\"1970-01-01T00:00:20Z\",\"level\":\"WARN\"}\n{\"message\":\"three\",\"timestamp\":\"1970-01-01T00:00:30Z\"}\n",
        )
        .unwrap();
        let index =
            crate::sources::index_file(raw.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        assert_eq!(index.lines.iter().map(|row| (row.ts, row.level)).collect::<Vec<_>>(),
            vec![(10_000, crate::model::LV_ERR), (20_000, crate::model::LV_WARN), (30_000, crate::model::LV_INFO)]);
        let state = AppState {
            source: RwLock::new(SourceData::None),
            source_publication: RwLock::new(Default::default()),
            source_names: RwLock::new(Vec::new()),
            codes: RwLock::new(Default::default()),
            system_codes: RwLock::new(Default::default()),
            derived: RwLock::new(Vec::new()),
            case_store_lock: Mutex::new(()),
            codes_path: Default::default(),
            system_codes_path: Default::default(),
        };
        let owner = analysis_runtime::capture(&state, Some(initial.identity()), Some(0), Mode::Publish)
            .unwrap();
        analysis_runtime::with(Some(owner), || {
            source_publication::publish(&state, index, vec!["fixture".into()], vec![], false)
        })
        .unwrap();
        let work = Work {
            cancelled: &|| false,
            progress: &|_, _, _| {},
        };
        let staged = {
            let source = state.source.read();
            let SourceData::Indexed(index) = &*source else {
                panic!()
            };
            let binding = crate::analysis_visibility::SourceSet::new(index).unwrap();
            exclusion_store::stage(
                directory.path(),
                Admission {
                    analysis: initial.identity(),
                    source_receipt: json!({}),
                },
                Purpose::Exclude,
                binding.descriptors(),
                [binding.member_row(index, 1, &Default::default())],
                &Default::default(),
                &work,
            )
            .unwrap()
        };
        let receipt = exclusion_store::publish(
            directory.path(),
            staged,
            "fixture",
            "",
            json!({}),
            &Default::default(),
            &work,
        )
        .unwrap();
        let admitted = analysis_runtime::capture(
            &state,
            Some(receipt.analysis_context.identity()),
            Some(source_publication::receipt_locked(&state).generation),
            Mode::Dataset,
        )
        .unwrap();
        admitted.validate(&state).unwrap();
        admitted.prepare_visibility(None).unwrap();
        session
            .conn()
            .unwrap()
            .execute_batch("DROP TABLE ev")
            .unwrap();
        analysis_runtime::with(Some(admitted), || {
            let captured = analysis_runtime::source(&state);
            let SourceData::Indexed(index) = &*captured else {
                panic!()
            };
            let codes = CodesConfig::default();
            let source = Source {
                idx: index,
                codes: &codes,
                system: &codes,
                derived: &[],
            };
            assert_eq!(count_session(&session, &source, &[]).unwrap(), 2);
            let gate = analysis_runtime::indexed_gate(index).unwrap().unwrap();
            let mask = gate.indexed_mask().expect("Dataset mask retains its exact positional capability");
            let plan = super::super::sparse_timeline::Plan::new(index, mask).unwrap().unwrap();
            assert!(sparse_timeline_session(&session, &source, mask, plan, 1, 40_000, 20_000, 2).unwrap().is_none());
            let store = directory.path().join("admitted-timeline.duckdb");
            let producer = duckdb::Connection::open_in_memory().unwrap();
            producer.execute_batch("SET threads=1; CREATE TABLE ev(lvl UTINYINT, ts BIGINT)").unwrap();
            for row in index.lines.iter() {
                producer.execute("INSERT INTO ev VALUES (?, ?)", duckdb::params![row.level, row.ts]).unwrap();
            }
            std::fs::write(store.with_extension("complete.json"), json!({"key":"admitted-timeline","rows":3,"version":5}).to_string()).unwrap();
            let identity = time_index::identity(&store, 3).unwrap();
            time_index::ensure(&producer, &store, &identity, &|| false).unwrap();
            *session.time_indexes.write() = Some(vec![time_index::open(&store, &identity).unwrap().unwrap()].into());
            let visible = sparse_timeline_session(&session, &source, mask, plan, 1, 40_000, 20_000, 2).unwrap().unwrap();
            assert_eq!((visible.total, visible.errors, visible.warnings), (2, 1, 0));
            std::fs::OpenOptions::new().write(true).open(time_index::path(&store)).unwrap()
                .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3)).unwrap();
            assert!(sparse_timeline_session(&session, &source, mask, plan, 1, 40_000, 20_000, 2).unwrap().is_none());
            assert!(session.exact_time_indexes().is_none());
            assert!(count_session(
                &session,
                &source,
                &filters(json!([{"column":"source","op":"equals_exact","value":"other"}]))
            )
            .is_err());
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(
                    directory
                        .path()
                        .join("exclusions-v1")
                        .join(format!("{}.sqlite3", receipt.batch_id)),
                )
                .unwrap();
            file.write_all(b"changed").unwrap();
            file.sync_all().unwrap();
            drop(file);
            assert!(
                count_session(&session, &source, &[]).is_err(),
                "a cached count must not bypass membership integrity"
            );
            assert!(sparse_timeline_session(&session, &source, mask, plan, 1, 40_000, 20_000, 2).is_err(),
                "optional temporal fallback must not bypass changed membership");
        });
    }

    #[test]
    fn sparse_visible_timeline_requires_no_session_but_keeps_source_and_cancellation_guards() {
        use crate::{analysis_context::Snapshot, analysis_visibility::{Cache, SourceSet}, exclusion_store::{self, Admission, Purpose, Work}};
        let directory = tempfile::tempdir().unwrap();
        crate::case_store::save_at(directory.path(), json!({"cases":[{"id":"sparse"}]})).unwrap();
        let initial: Snapshot = serde_json::from_value(
            crate::case_store::load_at(directory.path()).unwrap()["cases"][0]["analysisContext"].clone(),
        ).unwrap();
        let raw = directory.path().join("visible.jsonl");
        std::fs::write(&raw, "{\"timestamp\":\"1970-01-01T00:00:01Z\"}\n{\"timestamp\":\"1970-01-01T00:00:02Z\"}\n{\"timestamp\":\"1970-01-01T00:00:03Z\",\"level\":\"WARN\"}\n").unwrap();
        let index = crate::sources::index_file(raw.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        assert_eq!(index.lines.iter().map(|row| row.ts).collect::<Vec<_>>(), vec![1_000, 2_000, 3_000]);
        let work = Work { cancelled: &|| false, progress: &|_, _, _| {} };
        let binding = SourceSet::new(&index).unwrap();
        let staged = exclusion_store::stage(directory.path(), Admission {
            analysis: initial.identity(), source_receipt: json!({}),
        }, Purpose::Exclude, binding.descriptors(), [0, 1].map(|row|
            binding.member_row(&index, row, &Default::default())
        ), &Default::default(), &work).unwrap();
        let receipt = exclusion_store::publish(directory.path(), staged, "fixture", "", json!({}), &Default::default(), &work).unwrap();
        let identity = receipt.analysis_context.identity();
        let view = exclusion_store::visibility(directory.path(), &identity, &Default::default(), &work).unwrap();
        let mask = Cache::new().get_or_compile(&index, 1, &identity, &view, &Default::default(), &work).unwrap();
        let plan = super::super::sparse_timeline::Plan::new(&index, &mask).unwrap().unwrap();
        assert!(!plan.requires_base());
        let codes = CodesConfig::default();
        let source = Source { idx: &index, codes: &codes, system: &codes, derived: &[] };
        struct EngineRestore(bool);
        impl Drop for EngineRestore { fn drop(&mut self) { super::super::set_enabled(self.0); } }
        let _engine = EngineRestore(super::super::ENABLED.load(Ordering::SeqCst));
        super::super::set_enabled(false);
        assert!(ready_session(&source, true).unwrap().is_none());
        let result = sparse_timeline_query(&source, &mask, plan, 1, 4_000, 1_000, 4).unwrap().unwrap();
        assert_eq!((result.total, result.errors, result.warnings), (1, 0, 1));
        let token = crate::operations::token(Some("sparse-visible-no-session".into())).unwrap();
        crate::operations::cancel_id("sparse-visible-no-session");
        assert!(crate::operations::run_with_token(token, ||
            sparse_timeline_query(&source, &mask, plan, 1, 4_000, 1_000, 4)
        ).unwrap_err().contains("cancelad"));
        use std::io::Write;
        std::fs::OpenOptions::new().append(true).open(&raw).unwrap().write_all(b"changed").unwrap();
        assert!(sparse_timeline_query(&source, &mask, plan, 1, 4_000, 1_000, 4).is_err());
    }

    #[test]
    fn count_and_stats_use_complete_time_capability_with_exact_sql_parity() {
        let (_dir, session, _paths, readers) = fixture();
        let index = index();
        let config = CodesConfig::default();
        let source = Source { idx: &index, codes: &config, system: &config, derived: &[] };
        let cases = [json!([]),
            json!([{"column":"level","op":"equals_exact","value":"Informação"}]),
            json!([{"column":"level","op":"equals_exact","value":"Erro"}]),
            json!([{"column":"timestamp","op":"gt","value":"0"}]),
            json!([{"column":"timestamp","op":"lte","value":"0"}]),
            json!([{"column":"timestamp","op":"between","value":"-100","value2":"101"}]),
            json!([{"column":"timestamp","op":"gte","value":"0.5"},{"column":"timestamp","op":"lt","value":"101"}]),
            json!([{"column":"level","op":"equals_exact","value":"Erro"},{"column":"level","op":"equals_exact","value":"Aviso"}]),
            json!([{"column":"timestamp","op":"gt","value":"invalid"}])];
        let expectations: Vec<_> = cases.iter().map(|case| {
            let pfs = filters(case.clone());
            (count_session(&session, &source, &pfs).unwrap(), serde_json::to_value(stats_session(&session, &source, &pfs).unwrap()).unwrap())
        }).collect();
        *session.time_indexes.write() = Some(readers);
        // The only SQL table is gone: supported queries must use the verified
        // capability, not coincidentally pass by silently taking SQL fallback.
        session.conn().unwrap().execute_batch("DROP TABLE ev").unwrap();
        for (case, (count, stats)) in cases.iter().zip(expectations) {
            let pfs = filters(case.clone());
            assert_eq!(count_session(&session, &source, &pfs).unwrap(), count, "{case}");
            assert_eq!(serde_json::to_value(stats_session(&session, &source, &pfs).unwrap()).unwrap(), stats, "{case}");
        }
    }

    #[test]
    fn missing_or_unsupported_time_capability_keeps_exact_sql_fallback() {
        let (_dir, session, _paths, readers) = fixture();
        let index = index(); let config = CodesConfig::default();
        let source = Source { idx: &index, codes: &config, system: &config, derived: &[] };
        let pfs = filters(json!([{"column":"source","op":"equals_exact","value":"api"}]));
        let expected = serde_json::to_value(stats_session(&session, &source, &pfs).unwrap()).unwrap();
        *session.time_indexes.write() = Some(Arc::clone(&readers));
        assert_eq!(count_session(&session, &source, &pfs).unwrap(), 4);
        assert_eq!(serde_json::to_value(stats_session(&session, &source, &pfs).unwrap()).unwrap(), expected);
        let mut cache = time_index::VerifiedCache::default();
        cache.insert("part0".into(), Arc::clone(&readers[0]));
        *session.time_indexes.write() = cache.complete(["part0", "part1"]);
        assert!(session.exact_time_indexes().is_none());
        assert_eq!(count_session(&session, &source, &[]).unwrap(), 9);
    }

    #[test]
    fn changed_time_capability_falls_back_and_releases_only_its_stale_readers() {
        use fs2::FileExt;
        let (_dir, session, paths, readers) = fixture();
        let index = index(); let config = CodesConfig::default();
        let source = Source { idx: &index, codes: &config, system: &config, derived: &[] };
        let expected = serde_json::to_value(stats_session(&session, &source, &[]).unwrap()).unwrap();
        *session.time_indexes.write() = Some(readers);
        std::fs::OpenOptions::new().write(true).open(time_index::path(&paths[0])).unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3)).unwrap();
        assert_eq!(count_session(&session, &source, &[]).unwrap(), 9);
        assert!(session.exact_time_indexes().is_none());
        assert_eq!(serde_json::to_value(stats_session(&session, &source, &[]).unwrap()).unwrap(), expected);
        let lock = std::fs::OpenOptions::new().read(true).write(true).open(paths[0].with_extension("time.lock")).unwrap();
        FileExt::try_lock_exclusive(&lock).unwrap();
    }

    #[test]
    fn requested_timeline_uses_complete_capability_and_declines_stale_sets() {
        let (_dir, session, paths, readers) = fixture();
        assert!(timeline_session(&session, -100, 101, 51, 4).unwrap().is_none());
        *session.time_indexes.write() = Some(readers);
        session.conn().unwrap().execute_batch("DROP TABLE ev").unwrap();
        let result = timeline_session(&session, -100, 101, 51, 4).unwrap().unwrap();
        assert_eq!((result.total, result.errors, result.warnings), (6, 2, 0));
        assert_eq!(result.buckets.iter().map(|b| b.count).collect::<Vec<_>>(), vec![1, 1, 2, 2]);
        std::fs::OpenOptions::new().write(true).open(time_index::path(&paths[0])).unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3)).unwrap();
        assert!(timeline_session(&session, -100, 101, 51, 4).unwrap().is_none());
        assert!(session.exact_time_indexes().is_none());
    }

    #[test]
    fn grouped_timeline_sql_matches_bounded_canonical_fallback() {
        use crate::grouped_timeline::{Accumulator, Context, Grid, Spec};
        let (_dir, mut session, _paths, _readers) = fixture();
        let index = index(); let config = CodesConfig::default();
        let source = Source { idx: &index, codes: &config, system: &config, derived: &[] };
        session.schema.fields.insert("group".into(), "grouping".into());
        session.schema.lower.insert("group".into(), vec!["group".into()]);
        session.conn().unwrap().execute_batch("ALTER TABLE ev ADD COLUMN grouping VARCHAR").unwrap();
        let times = [Some(5),Some(-100),None,Some(0),Some(5),Some(101),Some(-1),Some(101),None];
        let keys = [Some("".to_string()),Some(" ".to_string()),None,Some("ignored".into()),Some("a".into()),Some("a".into()),Some("null".into()),Some("x".repeat(400)),Some("untimed".into())];
        let mut events = Vec::new();
        for (id, (timestamp, key)) in times.into_iter().zip(keys).enumerate() {
            session.conn().unwrap().execute("UPDATE ev SET grouping=? WHERE id=?", duckdb::params![key.as_deref(),id as i64]).unwrap();
            let mut event = Event::empty(); event.id=id;event.timestamp=timestamp;event.source=if id<4{"api"}else{"auth"}.into();
            if let Some(key)=key { event.fields.insert("group".into(),Value::String(key)); }
            events.push(event);
        }
        for field in ["group","GROUP","origem","absent"] {
            for filters in [vec![],vec![crate::query::Filter{column:"source".into(),op:"equals_exact".into(),value:"auth".into(),value2:None}]] {
                let pfs=prepared(&filters);
                for grid in [Grid{start:-100,bucket_ms:51,bucket_count:4},Grid{start:0,bucket_ms:1,bucket_count:6},Grid{start:0,bucket_ms:0,bucket_count:0}] {
                    let spec=Spec::new(field.into(),grid,Some(2),Context::default()).unwrap();
                    let mut expected=Accumulator::new(&spec).unwrap();
                    for event in &events { if pfs.iter().all(|filter|crate::query::matches_indexed(event,filter)) { expected.add_indexed_event(event).unwrap(); } }
                    assert_eq!(grouped_timeline_session(&session,&source,&pfs,&spec).unwrap().unwrap(),expected.finish().unwrap(),"{field} {grid:?}");
                }
            }
        }
        session.schema.structured.insert("nested".into());
        let spec=Spec::new("nested.name".into(),Grid{start:0,bucket_ms:10,bucket_count:1},None,Context::default()).unwrap();
        assert!(grouped_timeline_session(&session,&source,&[],&spec).unwrap().is_none());
    }

    #[test]
    fn grouped_timeline_limits_and_cancellation_are_errors_not_fallback() {
        use crate::grouped_timeline::{Context, Grid, Spec};
        let (_dir, session, _paths, _readers) = fixture();
        let index = index(); let config = CodesConfig::default();
        let source = Source { idx: &index, codes: &config, system: &config, derived: &[] };
        let spec=Spec::new("source".into(),Grid{start:-100,bucket_ms:51,bucket_count:4},None,Context::default()).unwrap();
        let limited=crate::resources::with_analytics_limit(128,||grouped_timeline_session(&session,&source,&[],&spec));
        assert!(limited.unwrap_err().contains("LOGINSIGHT_ANALYTICS_LIMIT_MB"));
        let token=crate::operations::token(Some("grouped-timeline-cancel".into())).unwrap();
        let result=crate::operations::run_with_token(token,||{
            crate::operations::cancel_id("grouped-timeline-cancel");
            assert!(grouped_timeline_session(&session,&source,&[],&spec).unwrap_err().contains("cancelad"));
        });
        assert!(result.unwrap_err().contains("cancelad"));
    }

    #[test]
    fn time_capability_cancellation_propagates_without_sql_fallback() {
        let (_dir, session, _paths, readers) = fixture();
        let index = index(); let config = CodesConfig::default();
        let source = Source { idx: &index, codes: &config, system: &config, derived: &[] };
        *session.time_indexes.write() = Some(readers);
        let token = crate::operations::token(Some("time-routing-cancel".into())).unwrap();
        let result = crate::operations::run_with_token(token, || {
            crate::operations::cancel_id("time-routing-cancel");
            assert!(count_session(&session, &source, &[]).unwrap_err().contains("cancelad"));
            assert!(stats_session(&session, &source, &[]).err().unwrap().contains("cancelad"));
            assert!(timeline_session(&session, -100, 100, 20, 11).unwrap_err().contains("cancelad"));
        });
        assert!(result.unwrap_err().contains("cancelad"));
        assert!(session.exact_time_indexes().is_some());
    }
}

// ------------------------------------------------------- grouped timeline

fn grouped_timeline_session(session: &Session, src: &Source, pfs: &[PreparedFilter], spec: &crate::grouped_timeline::Spec) -> Result<Option<crate::grouped_timeline::Response>> {
    use crate::grouped_timeline::{Response, Series};
    spec.validate()?;
    let mut budget = crate::query::AnalyticsBudget::new();
    budget.charge(spec.base_bytes())?;
    let mut names = false;
    let Some(key) = session.schema.grouped_field(&crate::querylang::field_ref(&spec.field), &mut names) else { return Ok(None); };
    let scope = scope(session, src, pfs)?;
    let projection = format!("SELECT {key} AS k, NULLIF(ts, 0) AS t FROM {} WHERE {}", scope.from(names), scope.cond);
    let start = spec.grid.start;
    let end = spec.grid.end();
    let inside = format!("t IS NOT NULL AND t::HUGEINT >= ({start})::HUGEINT AND t::HUGEINT < ({end})::HUGEINT");
    let mut result = Response::empty(spec)?;
    let mut expected = Vec::new();
    if spec.grid.bucket_count > 0 {
        stream_rows(session, &format!("SELECT k, count(*)::BIGINT AS n FROM ({projection}) WHERE ({inside}) AND k IS NOT NULL GROUP BY k ORDER BY n DESC, k ASC LIMIT {}", spec.limit), |row| {
            let key: String = row.get(0)?;
            budget.charge(spec.key_bytes(&key))?;
            expected.push(usize::try_from(row.get::<i64>(1)?).map_err(err)?);
            result.series.push(Series { key, count: 0, buckets: vec![0; spec.grid.bucket_count] });
            Ok(())
        })?;
    }
    let mut group = String::from("CASE WHEN t IS NULL THEN -3 WHEN NOT (");
    group.push_str(&inside); group.push_str(") THEN -4 WHEN k IS NULL THEN -1");
    for (index, series) in result.series.iter().enumerate() {
        group.push_str(&format!(" WHEN k = {} THEN {index}", lit(&series.key)));
    }
    group.push_str(" ELSE -2 END");
    // BIGINT subtraction can overflow before division. HUGEINT matches the
    // exact i128 half-open grid, including a final edge beyond i64::MAX.
    let bucket = if spec.grid.bucket_count == 0 { "0::BIGINT".into() } else {
        format!("CASE WHEN ({inside}) THEN ((t::HUGEINT - ({start})::HUGEINT) // {})::BIGINT ELSE 0::BIGINT END", spec.grid.bucket_ms)
    };
    let histogram = format!("SELECT g::BIGINT, b::BIGINT, count(*)::BIGINT FROM (SELECT {group} AS g, {bucket} AS b FROM ({projection})) GROUP BY g,b");
    stream_rows(session, &histogram, |row| {
        let group: i64 = row.get(0)?;
        let count = usize::try_from(row.get::<i64>(2)?).map_err(err)?;
        match group {
            -3 => result.untimed = result.untimed.checked_add(count).ok_or("Contagem temporal excedeu o limite.")?,
            -4 => result.outside_grid = result.outside_grid.checked_add(count).ok_or("Contagem temporal excedeu o limite.")?,
            group => {
                let bucket = usize::try_from(row.get::<i64>(1)?).map_err(err)?;
                match group {
                    -1 => result.missing.add(bucket, count)?,
                    -2 => result.other.add(bucket, count)?,
                    index => {
                        let series = usize::try_from(index).ok().and_then(|i|result.series.get_mut(i)).ok_or("Série temporal fora da seleção.")?;
                        let value = series.buckets.get_mut(bucket).ok_or("Faixa temporal fora da grade.")?;
                        *value = value.checked_add(count).ok_or("Contagem temporal excedeu o limite.")?;
                        series.count = series.count.checked_add(count).ok_or("Contagem temporal excedeu o limite.")?;
                    }
                }
                result.total.add(bucket, count)?;
            }
        }
        Ok(())
    })?;
    if result.series.iter().zip(expected).any(|(series,count)|series.count!=count) {
        return Err("A seleção temporal mudou durante o agrupamento.".into());
    }
    Ok(Some(result))
}

/// Fast path only when the engine proves the same canonical grouping key.
/// Unsupported fields fall back; budget, spill and cancellation failures do not.
pub(crate) fn grouped_timeline(src: &Source, pfs: &[PreparedFilter], spec: &crate::grouped_timeline::Spec) -> Result<Option<crate::grouped_timeline::Response>> {
    Ok(analytics_with(src, base_page_safe(pfs, &spec.field), |session| grouped_timeline_session(session, src, pfs, spec))?.flatten())
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
    let mut budget = crate::query::AnalyticsBudget::new();
    let mut groups: HashMap<Option<String>, Vec<Acc>> = HashMap::new();
    let mut order = Vec::new();
    stream_rows(scope.session, &sql, |r| {
        if groups.len() >= crate::query::MAX_GROUPS { return Err(AggResult::budget_error().error.unwrap()); }
        let key = stream_key(&r, 0, time)?;
        budget.group(&key, specs.len())?;
        let n = r.get::< i64>(1)? as u64;
        let mut column = 2;
        let mut accs = Vec::with_capacity(kinds.len());
        for (kind, spec) in kinds.iter().zip(specs) {
            accs.push(match kind {
                SpecSql::Count => Acc::Count(n),
                SpecSql::Distinct => {
                    column += 1;
                    Acc::Distinct(r.get::< i64>(column - 1)? as u64)
                }
                SpecSql::Numeric => {
                    let sum: Option<f64> = r.get(column)?;
                    let valid = r.get::< i64>(column + 1)? as u64;
                    let min: Option<f64> = r.get(column + 2)?;
                    let max: Option<f64> = r.get(column + 3)?;
                    let mut units = [0usize; 5];
                    for (u, slot) in units.iter_mut().enumerate() {
                        *slot = r.get::< i64>(column + 4 + u)? as usize;
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
        order.push(key.clone());
        groups.insert(key, accs);
        Ok(())
    })?;
    for (i, text) in texts {
        let sql = format!(
            "SELECT k, v FROM (SELECT k, v, row_number() OVER (PARTITION BY k ORDER BY id) AS rn \
             FROM (SELECT id, {key} AS k, {text} AS v FROM {from} WHERE {}) WHERE v <> '') \
             WHERE rn <= 100 ORDER BY k, rn",
            scope.cond
        );
        stream_rows(scope.session, &sql, |r| {
            let key = stream_key(&r, 0, time)?;
            let value: String = r.get(1)?;
            if let Some(Acc::StrAgg(list)) = groups.get_mut(&key).map(|accs| &mut accs[i]) {
                // Charge both retained strings and eventual joined output.
                budget.charge(value.len().saturating_mul(2).saturating_add(32))?;
                list.push(value);
            }
            Ok(())
        })?;
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
    let mut budget = crate::query::AnalyticsBudget::new();
    let (mut all_groups, mut all_records, mut retained) = (0, 0u64, 0u64);
    let mut groups = HashMap::new();
    let mut order = Vec::new();
    stream_rows(scope.session, &sql, |row| {
        let key = row.get::<Option<String>>(0)?;
        let count = row.get::<i64>(1)? as u64;
        all_groups = row.get::<i64>(2)? as usize;
        all_records = row.get::<i64>(3)? as u64;
        budget.group(&key, specs.len())?;
        retained += count;
        order.push(key.clone());
        groups.insert(key, specs.iter().map(|_| Acc::Count(count)).collect());
        Ok(())
    })?;
    let mut result = build_agg_result(groups, order, group, specs);
    result.omitted_groups = all_groups.saturating_sub(result.rows.len());
    result.omitted_records = all_records.saturating_sub(retained);
    Ok(result)
}

pub(crate) fn aggregate(src: &Source, pfs: &[PreparedFilter], group: &str, specs: &[AggSpec]) -> Result<Option<AggResult>> {
    let base_allowed = base_page_safe(pfs, group) && specs.iter().all(|spec| spec.func == "count" || base_field(&spec.column));
    analytics_with(src, base_allowed, |session| aggregate_of(&scope(session, src, pfs)?, group, specs))
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
) -> Result<Option<Vec<(String, AggResult)>>> {
    let base_allowed = base_page_safe(&prepared(filters), "") && columns.iter().all(|column| base_field(column));
    analytics_with(src, base_allowed, |session| {
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

const TIMESTAMP_SORT_SQL: &str = "COALESCE(ts, 0)";

struct SortKey {
    sql: String,
    desc: bool,
    text: bool,
    /// Only native timestamp keys may carry the immutable Session proof.
    timestamp_non_null: bool,
}

fn page_sort(scope: &Scope, column: &str, dir: &str) -> Result<(Vec<SortKey>, bool)> {
    let desc = dir == "desc";
    let int = |sql: String, desc| SortKey { sql, desc, text: false, timestamp_non_null: false };
    let mut names = false;
    let mut keys = match column {
        "" => vec![int("id".into(), false)],
        "id" => vec![int("id".into(), desc)],
        "timestamp" => {
            let mut key = int(TIMESTAMP_SORT_SQL.into(), desc);
            key.timestamp_non_null = scope.session.timestamps_non_null;
            vec![key]
        },
        "level" => vec![int("CAST(lvl AS BIGINT)".into(), desc)],
        column => {
            let value = scope.session.schema.column(column, &mut names).ok_or("Ordenação pelo texto bruto.")?;
            vec![
                int(format!("CAST(li_nkey({value}) IS NULL AS BIGINT)"), desc),
                int(format!("COALESCE(li_nkey({value}), 0)"), desc),
                SortKey { sql: format!("li_lower(COALESCE({value}, ''))"), desc, text: true, timestamp_non_null: false },
            ]
        }
    };
    if !matches!(column, "" | "id") {
        keys.push(int("id".into(), false));
    }
    Ok((keys, names))
}

/// The complete seek predicate and an optional proof used only for ordering.
struct PageSeek {
    predicate: String,
    timestamp_non_null: bool,
}

/// Lexicographic seek using non-null sort keys and the same final id tie-break
/// as the exact page API. Sort directions can differ (descending time, id asc).
fn after_cursor(sort: &[SortKey], keys: &[CursorKey]) -> Result<PageSeek> {
    if sort.is_empty() || keys.len() != sort.len() || keys.iter().zip(sort).any(|(key, sort)| matches!(key, CursorKey::Text(_)) != sort.text) {
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
    let exact = format!("({})", alternatives.join(" OR "));
    // The OR-ed lexicographic expression must remain authoritative, including
    // timestamp ties whose final id is always ascending. A redundant raw bound
    // lets DuckDB push the leading timestamp range into each segment scan.
    let bound = timestamp_cursor_bound(&sort[0], &keys[0]);
    let timestamp_non_null = bound.is_some();
    let predicate = match bound {
        Some(bound) => format!("({bound} AND {exact})"),
        None => exact,
    };
    Ok(PageSeek { predicate, timestamp_non_null })
}

fn timestamp_cursor_bound(sort: &SortKey, key: &CursorKey) -> Option<String> {
    // Match only the native timestamp key, never a user-defined numeric/text
    // expression. The id-only sort already has a directly pushable predicate.
    if sort.sql != TIMESTAMP_SORT_SQL { return None; }
    let CursorKey::Integer(value) = key else { return None; };
    // A Session proof excludes nulls in every range. Otherwise missing
    // timestamps sort as zero: optimize only when zero is strictly outside
    // the remaining leading range. An OR-is-null bound is correct but
    // does not improve the measured plan, so retain the original path there.
    let excludes_null = sort.timestamp_non_null || if sort.desc { *value < 0 } else { *value > 0 };
    excludes_null.then(|| format!("ts {} {value}", if sort.desc { "<=" } else { ">=" }))
}

#[cfg(test)]
mod interactive_contract_tests {
    use super::*;

    #[test]
    fn free_markers_are_not_replaced_inside_user_sql_literals() {
        let sql = "__li_free_7() OR value = 'O''Reilly __li_free_7()'";
        assert_eq!(replace_sql_marker(sql, "__li_free_7()", "id IN (SELECT id FROM chosen)").unwrap(),
            "id IN (SELECT id FROM chosen) OR value = 'O''Reilly __li_free_7()'");
        assert!(replace_sql_marker("'__li_free_7()'", "__li_free_7()", "FALSE").is_none());
    }

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
        let sort = vec![SortKey { sql: "value".into(), desc: true, text: true, timestamp_non_null: false }, SortKey { sql: "id".into(), desc: false, text: false, timestamp_non_null: false }];
        let clause = after_cursor(&sort, &[CursorKey::Text("O'Reilly".into()), CursorKey::Integer(7)]).unwrap().predicate;
        assert!(clause.contains("value < 'O''Reilly'"));
        assert!(clause.contains("value = 'O''Reilly' AND id > 7"));
        assert!(after_cursor(&sort, &[CursorKey::Integer(7), CursorKey::Integer(7)]).is_err());
        assert!(after_cursor(&sort, &[]).is_err());
    }

    #[test]
    fn timestamp_cursor_bounds_keep_null_zero_negative_and_extreme_keys() {
        let connection = duckdb::Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE ev(id BIGINT, ts BIGINT)").unwrap();
        let timestamps = [Some(i64::MIN), Some(-7), Some(-1), None, Some(0),
            Some(0), None, Some(1), Some(7), Some(i64::MAX)];
        for (id, ts) in timestamps.iter().enumerate() {
            connection.execute("INSERT INTO ev VALUES (?, ?)", duckdb::params![id as i64, ts]).unwrap();
        }
        for desc in [false, true] {
            let sort = vec![
                SortKey { sql: TIMESTAMP_SORT_SQL.into(), desc, text: false, timestamp_non_null: false },
                SortKey { sql: "id".into(), desc: false, text: false, timestamp_non_null: false },
            ];
            for timestamp in [i64::MIN, -8, -7, -1, 0, 1, 7, 8, i64::MAX] {
                for id in [-1, 3, 5, 10] {
                    let keys = [CursorKey::Integer(timestamp), CursorKey::Integer(id)];
                    let seek = after_cursor(&sort, &keys).unwrap();
                    let excludes_null = if desc { timestamp < 0 } else { timestamp > 0 };
                    assert_eq!(seek.timestamp_non_null, excludes_null);
                    assert_eq!(timestamp_cursor_bound(&sort[0], &keys[0]).is_some(), excludes_null);
                    let order = page_order(&sort, &seek);
                    assert_eq!(order, format!("{} {}, id ASC", if excludes_null { "ts" } else { TIMESTAMP_SORT_SQL }, if desc { "DESC" } else { "ASC" }));
                    let sql = format!("SELECT id FROM ev WHERE {} ORDER BY {order}", seek.predicate);
                    let mut statement = connection.prepare(&sql).unwrap();
                    let actual = statement.query_map([], |row| row.get::<_, i64>(0)).unwrap()
                        .collect::<duckdb::Result<Vec<_>>>().unwrap();
                    let mut expected = timestamps.iter().enumerate().map(|(id, ts)| (ts.unwrap_or(0), id as i64))
                        .filter(|(ts, row)| (if desc { *ts < timestamp } else { *ts > timestamp }) || (*ts == timestamp && *row > id))
                        .collect::<Vec<_>>();
                    expected.sort_unstable_by(|a, b| {
                        let time = a.0.cmp(&b.0);
                        (if desc { time.reverse() } else { time }).then(a.1.cmp(&b.1))
                    });
                    assert_eq!(actual, expected.into_iter().map(|(_, id)| id).collect::<Vec<_>>(), "{sql}");
                }
            }
        }
        assert!(after_cursor(&[], &[]).is_err());
    }

    #[test]
    fn id_cursor_remains_a_direct_native_bound_in_both_directions() {
        for desc in [false, true] {
            let sort = [SortKey { sql: "id".into(), desc, text: false, timestamp_non_null: false }];
            assert_eq!(after_cursor(&sort, &[CursorKey::Integer(42)]).unwrap().predicate,
                format!("((id {} 42))", if desc { "<" } else { ">" }));
        }
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

fn page_fingerprint(src: &Source, pfs: &[PreparedFilter], sort_column: &str, sort_dir: &str, base_safe: bool) -> Result<String> {
    let source_key = super::spec(src.idx, src.codes, src.system, if base_safe { &[] } else { src.derived })
        .ok_or("Fonte indisponível para paginação.")?.key;
    let filters: Vec<_> = pfs.iter().map(|pf| &pf.f).collect();
    let mut hash = Sha256::new();
    hash.update(source_key);
    hash.update(crate::analysis_runtime::cache_namespace());
    hash.update(super::catalogs_signature(src.codes, src.system));
    hash.update(serde_json::to_vec(&(filters, sort_column, sort_dir)).map_err(err)?);
    Ok(format!("{:x}", hash.finalize()))
}

fn page_seek(cursor: Option<&str>, sort: &[SortKey], fingerprint: &str, rows: usize, offset: usize) -> Result<(PageSeek, usize)> {
    let Some(value) = cursor else { return Ok((PageSeek { predicate: "TRUE".into(), timestamp_non_null: false }, offset)); };
    if value.len() > 1_100_000 {
        return Err("PAGINATION_RESET_REQUIRED: Cursor de paginação excede o limite.".into());
    }
    let cursor: PageCursor = serde_json::from_str(value).map_err(|_| "PAGINATION_RESET_REQUIRED: Cursor de paginação inválido.".to_string())?;
    if cursor.version != 1 || cursor.fingerprint != fingerprint {
        return Err("PAGINATION_RESET_REQUIRED: A consulta mudou. Recarregue a primeira página.".into());
    }
    if cursor.position > rows {
        return Err("PAGINATION_RESET_REQUIRED: Posição de paginação inválida.".into());
    }
    Ok((after_cursor(sort, &cursor.keys)?, cursor.position))
}

fn page_order(sort: &[SortKey], seek: &PageSeek) -> String {
    sort.iter().enumerate().map(|(i, key)| {
        // Once the Session or cursor proves timestamps are non-null, the raw
        // column has exactly the same order as COALESCE. Keeping it visible to
        // TOP_N enables DuckDB's dynamic range filter on each segment scan.
        let sql = if i == 0 && (seek.timestamp_non_null || key.timestamp_non_null) && key.sql == TIMESTAMP_SORT_SQL { "ts" } else { &key.sql };
        format!("{sql} {}", if key.desc { "DESC" } else { "ASC" })
    }).collect::<Vec<_>>().join(", ")
}

fn page_statement(scope: &Scope, sort: &[SortKey], names: bool, seek: &PageSeek, batch: usize, offset: usize) -> String {
    let order = page_order(sort, seek);
    let projection = sort.iter().map(|key| key.sql.as_str()).collect::<Vec<_>>().join(", ");
    format!("SELECT id, {projection} FROM {} WHERE ({}) AND {} ORDER BY {order} LIMIT {batch} OFFSET {offset}", scope.from(names), scope.cond, seek.predicate)
}

/// A required top-level equality bounds the entire conjunction. Only a
/// complete, canonically verified empty/singleton set may bypass page SQL;
/// free-text OR/NOT terms never establish this proof.
fn required_hex_page(session: &Session, src: &Source, pfs: &[PreparedFilter], plan: &super::sql::Plan) -> Result<Option<crate::query::SelectedPage>> {
    // Planner invariant: hex_fields contains required top-level equals_exact
    // clauses only. OR/NOT query-expression terms must never enter this list.
    if !pfs.iter().any(|pf| pf.f.op == "equals_exact" && super::text::exact_field_hex_word(&pf.f.value).is_some()) {
        return Ok(None);
    }
    let gate = crate::analysis_runtime::indexed_gate(src.idx)?;
    for term in &plan.tests.hex_fields {
        let Some(selected) = exact_hex_selection(session, src, term)? else { continue; };
        if !selected.known_empty && selected.single_id.is_none() { continue; }
        let mut ids = Vec::new();
        if let Some(id) = selected.single_id.filter(|&id| gate.as_ref().is_none_or(|gate| gate.allows_known_row(id))) {
            crate::operations::check()?;
            let event = src.event(id);
            if pfs.iter().all(|pf| crate::query::matches_indexed(&event, pf)) {
                ids.push(id);
            }
        }
        crate::operations::check()?;
        return Ok(Some(crate::query::SelectedPage {
            total: Some(ids.len()), ids, has_more: false, next_cursor: None,
            engine: "columnar".into(), warning: None,
        }));
    }
    Ok(None)
}

/// Optional backend diagnostics, separate from latency benchmark samples.
/// No source values or filters are logged. Candidate evaluation includes the
/// residual matcher, cancellation checkpoints and bounded page bookkeeping.
struct PageTrace {
    started: std::time::Instant,
    scope_ms: f64,
    sql_ms: f64,
    candidate_ms: f64,
    cache_hit: bool,
    singleton: bool,
    term_selections: usize,
    sql_batches: usize,
    sql_candidates: usize,
    residual_candidates: usize,
    returned: usize,
    success: bool,
}

fn page_trace_start() -> Option<std::time::Instant> {
    std::env::var_os("LOGINSIGHT_PAGE_TRACE").map(|_| std::time::Instant::now())
}

impl PageTrace {
    fn new() -> Option<Self> {
        page_trace_start().map(|started| Self {
            started, scope_ms: 0.0, sql_ms: 0.0, candidate_ms: 0.0,
            cache_hit: false, singleton: false, term_selections: 0,
            sql_batches: 0, sql_candidates: 0, residual_candidates: 0,
            returned: 0, success: false,
        })
    }
}

impl Drop for PageTrace {
    fn drop(&mut self) {
        eprintln!("PAGE_TRACE {}", serde_json::json!({
            "phase": "selection", "operationId": crate::operations::current_id(),
            "success": self.success, "selectionMs": self.started.elapsed().as_secs_f64() * 1000.0,
            "scopeMs": self.scope_ms, "sqlMs": self.sql_ms, "candidateEvaluationMs": self.candidate_ms,
            "completeSelectionHit": self.cache_hit, "singleton": self.singleton,
            "termSelectionsHeld": self.term_selections, "sqlBatches": self.sql_batches,
            "sqlCandidates": self.sql_candidates, "residualCandidates": self.residual_candidates,
            "returnedRows": self.returned,
        }));
    }
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
    select_page(src, pfs, sort_column, sort_dir, offset, limit, cursor).map(|result| {
        let selected = result?;
        let started = page_trace_start();
        let rows = page_rows(src, selected.ids.clone())?;
        crate::operations::check()?;
        if let Some(started) = started {
            eprintln!("PAGE_TRACE {}", serde_json::json!({
                "phase": "hydration", "operationId": crate::operations::current_id(),
                "hydrationMs": started.elapsed().as_secs_f64() * 1000.0, "hydratedRows": rows.len(),
            }));
        }
        Ok(selected.into_full(rows))
    })
}

pub(crate) fn query_projected_page(
    src: &Source, pfs: &[PreparedFilter], sort_column: &str, sort_dir: &str,
    offset: usize, cursor: Option<&str>, plan: &crate::page_projection::ProjectionPlan,
) -> Option<Result<crate::page_projection::ProjectedPage>> {
    select_page(src, pfs, sort_column, sort_dir, offset, plan.limit, cursor).map(|result| {
        let selected = result?;
        let items = selected.ids.iter().map(|&id| crate::page_projection::project_indexed_row(
            src.idx, id, src.codes, src.system, src.derived, plan,
        )).collect::<Result<Vec<_>>>()?;
        crate::operations::check()?;
        plan.finish(selected, items)
    })
}

fn select_page(
    src: &Source, pfs: &[PreparedFilter], sort_column: &str, sort_dir: &str,
    offset: usize, limit: usize, cursor: Option<&str>,
) -> Option<Result<crate::query::SelectedPage>> {
    for part in &src.idx.parts {
        if let Err(error) = crate::sources::validate_source(part) { return Some(Err(error)); }
    }
    if sort_column == "raw" { return None; }
    let limit = limit.clamp(1, 2_000);
    let base_safe = base_page_safe(pfs, sort_column);
    let session = match ready_session(src, base_safe) { Ok(Some(session)) => session, Ok(None) => return None, Err(error) => return Some(Err(error)) };
    let _names = match session.names_guard(src.codes, src.system) { Ok(guard) => guard, Err(error) => return Some(Err(error)) };
    Some((|| {
        let mut trace = PageTrace::new();
        crate::operations::check()?;
        let (scope, verify) = match page_scope(&session, src, pfs, cursor.is_none() && offset == 0)? {
            PageScope::Singleton(page) => {
                if let Some(trace) = trace.as_mut() {
                    trace.scope_ms = trace.started.elapsed().as_secs_f64() * 1000.0;
                    trace.singleton = true;
                    trace.returned = page.ids.len();
                    trace.success = true;
                }
                return Ok(page);
            }
            PageScope::Planned { scope, verify } => (scope, verify),
        };
        if let Some(trace) = trace.as_mut() {
            trace.scope_ms = trace.started.elapsed().as_secs_f64() * 1000.0;
            trace.cache_hit = scope._selection.is_some();
            trace.term_selections = scope._free.len();
        }
        let fingerprint = page_fingerprint(src, pfs, sort_column, sort_dir, base_safe)?;
        let exact = verify.is_empty();
        let verifier = if exact { None } else {
            Some(crate::query::CandidateVerifier::new(src.idx, pfs, &[], &verify, src.codes, src.system, src.derived)?)
        };
        let (sort, names) = page_sort(&scope, sort_column, sort_dir)?;
        let (after, position) = page_seek(cursor, &sort, &fingerprint, src.idx.lines.len(), offset)?;
        let mut offset_sql = if cursor.is_some() || !exact { 0 } else { offset };
        let mut skip_matches = if cursor.is_none() && !exact { offset } else { 0 };
        let mut seek = after;
        let mut found = Vec::new();
        let batch_size = if exact { limit.saturating_add(1) } else { limit.saturating_add(1).max(1_024) };
        loop {
            crate::operations::check()?;
            let sql = page_statement(&scope, &sort, names, &seek, batch_size, offset_sql);
            let sql_started = trace.as_ref().map(|_| std::time::Instant::now());
            let candidates = rows(&session, &sql, |row| {
                let id = row.get::<_, i64>(0)? as usize;
                let keys = sort.iter().enumerate().map(|(i, key)| {
                    if key.text { row.get::<_, String>(i + 1).map(CursorKey::Text) }
                    else { row.get::<_, i64>(i + 1).map(CursorKey::Integer) }
                }).collect::<duckdb::Result<Vec<_>>>()?;
                Ok((id, keys))
            })?;
            if let (Some(trace), Some(started)) = (trace.as_mut(), sql_started) {
                trace.sql_ms += started.elapsed().as_secs_f64() * 1000.0;
                trace.sql_batches += 1;
                trace.sql_candidates += candidates.len();
            }
            let candidate_started = trace.as_ref().map(|_| std::time::Instant::now());
            let exhausted = candidates.len() < batch_size;
            let last_keys = candidates.last().map(|(_, keys)| keys.clone());
            for candidate in candidates {
                crate::operations::check()?;
                if let Some(verifier) = &verifier {
                    if let Some(trace) = trace.as_mut() { trace.residual_candidates += 1; }
                    if !verifier.matches(candidate.0) { continue; }
                }
                if skip_matches > 0 { skip_matches -= 1; continue; }
                found.push(candidate);
                if found.len() > limit { break; }
            }
            if let (Some(trace), Some(started)) = (trace.as_mut(), candidate_started) {
                trace.candidate_ms += started.elapsed().as_secs_f64() * 1000.0;
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
        let total = if pfs.is_empty() { Some(crate::analysis_runtime::visible_total(src.idx)?) }
            else if !has_more && (position == 0 || !found.is_empty()) { Some(position.saturating_add(found.len())) }
            else { None };
        let ids: Vec<_> = found.into_iter().map(|(id, _)| id).collect();
        crate::operations::check()?;
        if let Some(trace) = trace.as_mut() {
            trace.returned = ids.len();
            trace.success = true;
        }
        Ok(crate::query::SelectedPage {
            ids, total, has_more, next_cursor, engine: "columnar".into(),
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
    explain_page_at(src, pfs, sort_column, sort_dir, 0, limit, None, analyze)
}

pub(crate) fn explain_page_at(
    src: &Source, pfs: &[PreparedFilter], sort_column: &str, sort_dir: &str,
    offset: usize, limit: usize, cursor: Option<&str>, analyze: bool,
) -> Option<Result<Value>> {
    let base_safe = base_page_safe(pfs, sort_column);
    let session = match ready_session(src, base_safe) { Ok(Some(session)) => session, Ok(None) => return None, Err(error) => return Some(Err(error)) };
    let _names = match session.names_guard(src.codes, src.system) { Ok(guard) => guard, Err(error) => return Some(Err(error)) };
    Some((|| {
        let (scope, verify) = match page_scope(&session, src, pfs, sort_column != "raw" && cursor.is_none() && offset == 0)? {
            PageScope::Singleton(page) => {
                return Ok(serde_json::json!({ "mode": "verified_singleton", "sql": null,
                    "plan": "Complete typed equality candidates verified directly; no page SQL statement executed.",
                    "profile": null, "analyzed": false, "exactPredicate": true, "total": page.total,
                    "note": "Index candidate discovery and canonical event verification are not DuckDB page scans." }));
            }
            PageScope::Planned { scope, verify, .. } => (scope, verify),
        };
        let fingerprint = page_fingerprint(src, pfs, sort_column, sort_dir, base_safe)?;
        let exact = verify.is_empty();
        let (sort, names) = page_sort(&scope, sort_column, sort_dir)?;
        let (seek, _) = page_seek(cursor, &sort, &fingerprint, src.idx.lines.len(), offset)?;
        let limit = limit.clamp(1, 2_000);
        let batch = if exact { limit + 1 } else { (limit + 1).max(1_024) };
        let sql = page_statement(&scope, &sort, names, &seek, batch, if cursor.is_some() || !exact { 0 } else { offset });
        let plan = rows(&session, &format!("EXPLAIN {}{sql}", if analyze { "ANALYZE " } else { "" }), |row| row.get::<_, String>(1))?.join("\n");
        let profile = if analyze {
            let value = rows(&session, &format!("EXPLAIN (ANALYZE, FORMAT JSON) {sql}"), |row| row.get::<_, String>(1))?.join("\n");
            Some(serde_json::from_str::<Value>(&value).map_err(err)?)
        } else { None };
        Ok(serde_json::json!({ "sql": sql, "plan": plan, "profile": profile, "analyzed": analyze, "exactPredicate": exact,
            "candidateBatchSize": batch, "note": "Explains the actual first candidate batch; candidate discovery and event hydration are outside this SQL profile." }))
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
fn page_rows(src: &Source, ids: Vec<usize>) -> Result<Vec<Event>> {
    let binding = crate::analysis_runtime::source_set(src.idx)?;
    crate::global_scheduler::map(ids, |i| {
            let mut event = src.event(i);
            crate::analysis_runtime::attach_provenance_with(src.idx, &binding, &mut event)?;
            crate::entities::annotate(&mut event);
            // A page excludes raw text. Clearing it would keep the entire input
            // allocation alive alongside fields until response serialization.
            event.raw = String::new();
            Ok(event)
        }).into_iter().collect()
}

#[cfg(test)]
mod page_payload_tests {
    use super::*;

    #[test]
    fn projected_columnar_pages_share_exact_cursor_and_singleton_selection() {
        struct Restore(Option<std::ffi::OsString>, bool);
        impl Drop for Restore {
            fn drop(&mut self) {
                match &self.0 { Some(value) => std::env::set_var("LOGINSIGHT_ENGINE_DIR", value), None => std::env::remove_var("LOGINSIGHT_ENGINE_DIR") }
                crate::engine::set_enabled(self.1);
            }
        }
        let _restore = Restore(std::env::var_os("LOGINSIGHT_ENGINE_DIR"), super::super::enabled());
        let directory = tempfile::tempdir().unwrap();
        std::env::set_var("LOGINSIGHT_ENGINE_DIR", directory.path().join("engine"));
        crate::engine::set_enabled(true);
        let path = directory.path().join("projected-pages.jsonl");
        let records: Vec<_> = (0..6).map(|i| serde_json::json!({
            "message": format!("record {i}"), "timestamp": ([Some("1969-12-31T23:59:59.999Z"), None, Some("1970-01-01T00:00:00Z")][i % 3]),
            "request_id": format!("{i:032x}"), "body": "日".repeat(24_000),
        }).to_string()).collect();
        std::fs::write(&path, records.join("\n")).unwrap();
        let index = crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let config = CodesConfig::default();
        crate::engine::prepare(&index, &config, &config, &[], &|_, _| {}).unwrap();
        let source = Source { idx: &index, codes: &config, system: &config, derived: &[] };
        let plan = crate::page_projection::ProjectionPlan::new(crate::page_projection::ProjectionRequest {
            projection_version: 1, columns: vec!["timestamp".into(), "body".into()], cell_bytes: None, response_bytes: None,
        }, 2, crate::page_projection::Receipt {
            analysis_context: crate::analysis_context::Identity { case_id: "test".into(), analysis_id: "analysis".into(), config_revision: 1, visibility_revision: 0 },
            source_generation: Some(1), case_key: None,
            case_content_token: None,
            catalog_signature: crate::engine::catalog_content_signature(&CodesConfig::default(), &CodesConfig::default()),
            catalog_epoch: crate::engine::catalog_token(&CodesConfig::default(), &CodesConfig::default()).epoch,
        }).unwrap();
        for pfs in [Vec::new(), crate::query::prepare(&[crate::query::Filter { column: "request_id".into(), op: "equals_exact".into(), value: format!("{:032x}", 3), value2: None }])] {
            for direction in ["asc", "desc"] {
                let mut cursor: Option<String> = None;
                let mut offset = 0;
                loop {
                    let full = query_page(&source, &pfs, "timestamp", direction, offset, 2, cursor.as_deref()).unwrap().unwrap();
                    let projected = query_projected_page(&source, &pfs, "timestamp", direction, offset, cursor.as_deref(), &plan).unwrap().unwrap();
                    assert_eq!(projected.items.iter().map(|r| r.row.id).collect::<Vec<_>>(), full.rows.iter().map(|e| e.id).collect::<Vec<_>>());
                    assert_eq!(projected.total, full.total);
                    assert_eq!(projected.has_more, full.has_more);
                    assert_eq!(projected.next_cursor, full.next_cursor);
                    assert_eq!(projected.warning, full.warning);
                    if !full.has_more { break; }
                    offset += full.rows.len();
                    cursor = full.next_cursor;
                }
            }
        }
    }

    #[test]
    fn cached_and_residual_pages_preserve_canonical_order_cursors_and_generated_messages() {
        struct Restore(Option<std::ffi::OsString>, bool);
        impl Drop for Restore {
            fn drop(&mut self) {
                match &self.0 { Some(value) => std::env::set_var("LOGINSIGHT_ENGINE_DIR", value), None => std::env::remove_var("LOGINSIGHT_ENGINE_DIR") }
                crate::engine::set_enabled(self.1);
            }
        }
        let _restore = Restore(std::env::var_os("LOGINSIGHT_ENGINE_DIR"), super::super::enabled());
        let directory = tempfile::tempdir().unwrap();
        std::env::set_var("LOGINSIGHT_ENGINE_DIR", directory.path().join("engine"));
        crate::engine::set_enabled(true);
        let path = directory.path().join("residual-navigation.jsonl");
        let mut records: Vec<_> = (0..1099).map(|id| serde_json::json!({
            "timestamp": ([Some("1969-12-31T23:59:59.999Z"), None, Some("1970-01-01T00:00:00Z"), Some("2026-01-01T00:00:00Z")][id % 4]),
            "source": "api", "code": "42", "message": if id % 5 == 0 { "needle" } else { "ordinary" },
            "keep": id == 0 || id == 1098, "User": "upper", "user": if id % 2 == 0 { "lower" } else { "other" },
            "items": [{"kind": if id % 17 == 0 { "rare" } else { "common" }}],
        }).to_string()).collect();
        // The parser adds "negado" to the canonical message; it does not occur
        // in the raw JSON. A raw-only miss is therefore not a page proof.
        records.push(serde_json::json!({"eventSource":"ec2.amazonaws.com", "eventName":"RunInstances", "errorCode":"Denied"}).to_string());
        std::fs::write(&path, records.join("\n")).unwrap();
        let index = crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let config: CodesConfig = serde_json::from_value(serde_json::json!({"api":{"42":{"name":"catalogneedle","description":"enriched"}}})).unwrap();
        let derived = vec![CompiledDerived {
            name: "derived_user".into(), source: "user".into(),
            rules: vec![crate::sources::CompiledRule::new(regex::Regex::new("^(.*)$").unwrap(), None, None).unwrap()],
            steps: Vec::new(), lookup: None,
        }];
        crate::engine::prepare(&index, &config, &config, &derived, &|_, _| {}).unwrap();
        let source = Source { idx: &index, codes: &config, system: &config, derived: &derived };
        let session = super::super::session_checked(&index, &config, &config, &derived).unwrap().unwrap();
        let filter = |column: &str, op: &str, value: &str| crate::query::Filter {
            column: column.into(), op: op.into(), value: value.into(), value2: None,
        };
        let raw = filter("raw", "contains", "\"keep\":true");
        let workloads = vec![
            vec![filter("id", "gte", "0"), raw.clone()],
            vec![filter("_all", "query", "items.0.kind:rare AND User:upper")],
            vec![filter("_all", "query", "catalogneedle"), raw.clone()],
            vec![filter("derived_user", "equals_exact", "lower"), raw.clone()],
            vec![filter("_all", "contains", " needle ")],
            vec![filter("_all", "contains", "negado")],
            vec![filter("_all", "not_contains", "negado")],
            vec![filter("_all", "regex", "^needle\\n")],
        ];
        let events: Vec<_> = (0..index.lines.len()).map(|id| source.event(id)).collect();
        assert!(events.last().unwrap().message.contains("negado"));
        assert!(!events.last().unwrap().raw.contains("negado"));
        for filters in workloads {
            let pfs = prepared(&filters);
            let plan = session.schema.plan(&pfs);
            assert!(!plan.exact(), "fixture must exercise residual confirmation: {}", serde_json::to_string(&filters).unwrap());
            let reuse_safe = plan.lines.is_empty();
            for sort in ["id", "timestamp", "user"] {
                for direction in ["asc", "desc"] {
                    session.selections.lock().clear();
                    session.collect_garbage().unwrap();
                    let mut expected: Vec<_> = events.iter().filter(|event| pfs.iter().all(|pf| crate::query::matches_indexed(event, pf))).map(|event| event.id).collect();
                    match sort {
                        "id" => if direction == "desc" { expected.reverse(); },
                        "timestamp" => expected.sort_by(|&a, &b| {
                            let order = index.lines.at(a).ts.cmp(&index.lines.at(b).ts);
                            (if direction == "desc" { order.reverse() } else { order }).then(a.cmp(&b))
                        }),
                        _ => crate::query::sort_indices(&events, &mut expected, sort, direction == "desc"),
                    }
                    // Sparse id ordering must continue beyond the 1,024-row
                    // candidate batch to establish the second match.
                    let limit = if filters[0].column == "id" { 1 } else { 31 };
                    let mut offset = 0;
                    let mut cursor: Option<String> = None;
                    let mut cold = Vec::new();
                    loop {
                        let page = select_page(&source, &pfs, sort, direction, offset, limit, cursor.as_deref()).unwrap().unwrap();
                        assert_eq!(page.ids, expected.iter().skip(offset).take(limit).copied().collect::<Vec<_>>(), "cold {sort} {direction}");
                        assert_eq!(page.has_more, offset + page.ids.len() < expected.len());
                        let next = page.next_cursor.clone();
                        let more = page.has_more;
                        cold.push((offset, cursor, page));
                        if !more { break; }
                        offset += limit;
                        cursor = next;
                    }
                    let key = complete_selection_key(&session, &pfs).unwrap();
                    assert!(session.cached_selection(&key).is_none(), "cold paging must not materialize a complete selection");
                    let random_offset = expected.len().min(13);
                    let random_cold = select_page(&source, &pfs, sort, direction, random_offset, limit, None).unwrap().unwrap();
                    let count = count_session(&session, &source, &pfs).unwrap();
                    if reuse_safe {
                        assert_eq!(count, expected.len());
                        assert!(session.cached_selection(&key).is_some());
                    }
                    let PageScope::Planned { scope, verify, .. } = page_scope(&session, &source, &pfs, false).unwrap() else { panic!("not a singleton") };
                    assert_eq!(scope._selection.is_some(), reuse_safe);
                    assert_eq!(verify.is_empty(), reuse_safe);
                    drop(scope);
                    // Reverse replay covers previous-page navigation; each
                    // cursor was minted before the completed selection existed.
                    for (offset, cursor, cold_page) in cold.iter().rev() {
                        let warm = select_page(&source, &pfs, sort, direction, *offset, limit, cursor.as_deref()).unwrap().unwrap();
                        assert_eq!(warm.ids, cold_page.ids, "warm {sort} {direction}");
                        assert_eq!(warm.total, cold_page.total);
                        assert_eq!(warm.has_more, cold_page.has_more);
                        assert_eq!(warm.next_cursor, cold_page.next_cursor);
                    }
                    let random_warm = select_page(&source, &pfs, sort, direction, random_offset, limit, None).unwrap().unwrap();
                    assert_eq!(random_warm.ids, random_cold.ids);
                    assert_eq!(random_warm.next_cursor, random_cold.next_cursor);
                    if let Some(cursor) = cold[0].2.next_cursor.as_deref() {
                        let changed = prepared(&[filter("raw", "contains", "changed")]);
                        let error = select_page(&source, &changed, sort, direction, 0, limit, Some(cursor)).unwrap().err().unwrap();
                        assert!(error.starts_with("PAGINATION_RESET_REQUIRED:"));
                    }
                }
            }
        }
    }

    #[test]
    fn hydrated_page_releases_raw_capacity_and_preserves_full_evidence_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("wide-page.jsonl");
        let body = "日".repeat(24_000);
        let raw = serde_json::to_string(&serde_json::json!({
            "message":"request", "payload":body, "encoded":"日", "code":200
        })).unwrap();
        std::fs::write(&path, format!("{raw}\n")).unwrap();
        let index = crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let config = CodesConfig::default();
        // Legacy regex overlays may replace a field while retaining its exact
        // original. Typed transform pipelines deliberately reject that collision.
        let derived = vec![CompiledDerived {
            name:"payload".into(), source:"encoded".into(),
            rules:vec![crate::sources::CompiledRule::new(regex::Regex::new("^(.*)$").unwrap(), None, None).unwrap()], steps:Vec::new(), lookup:None,
        }];
        let source = Source { idx:&index, codes:&config, system:&config, derived:&derived };
        let original = source.event(0);
        assert!(original.raw.capacity() >= body.len());
        let rows = page_rows(&source, vec![0]).unwrap();
        assert_eq!(rows.len(), 1);
        let event = &rows[0];
        assert!(event.raw.is_empty());
        assert_eq!(event.raw.capacity(), 0, "raw must not retain the source record buffer");
        assert_eq!(event.fields["payload"], Value::from("日"));
        assert!(matches!(event.derived_originals.get("payload"), Some(crate::model::DerivedOriginal::Present(Value::String(value))) if value == &body));
        assert_eq!(serde_json::to_value(&event.derived_originals).unwrap(), serde_json::to_value(&original.derived_originals).unwrap());
        assert_eq!(event.event_ref, original.event_ref);
        assert!(event.evidence_provenance.is_some());
        let mut expected = original;
        crate::analysis_runtime::attach_provenance(&index, &mut expected).unwrap();
        crate::entities::annotate(&mut expected);
        expected.raw.clear();
        assert_eq!(serde_json::to_value(event).unwrap(), serde_json::to_value(expected).unwrap());
        let recovery = crate::query::query_page_lines(&index, &[], "id", "asc", 0, 1, &config, &config, &derived).unwrap();
        assert_eq!(recovery.rows.len(), 1);
        assert_eq!(recovery.rows[0].raw.capacity(), 0);
        assert_eq!(serde_json::to_value(&recovery.rows[0]).unwrap(), serde_json::to_value(event).unwrap());
        assert_eq!(source.event(0).raw, raw, "page hydration must not mutate original detail/evidence data");
    }
}

pub(crate) fn query(
    src: &Source,
    pfs: &[PreparedFilter],
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
) -> Result<Option<QueryResult>> {
    analytics_with(src, false, |session| {
        let scope = scope(session, src, pfs)?;
        let (total, ids) = page_ids(&scope, sort_column, sort_dir, offset, limit)?;
        Ok(QueryResult {
            total,
            rows: page_rows(src, ids)?,
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
) -> Result<Option<ExplorerSnapshot>> {
    analytics_with(src, false, |session| {
        let scope = scope(session, src, pfs)?;
        let stats = stats_of(&scope)?;
        let count = count_spec();
        let sources = aggregate_of(&scope, "source", &count)?;
        let codes = aggregate_of(&scope, "code", &count)?;
        let (total, ids) = page_ids(&scope, sort_column, sort_dir, offset, limit)?;
        Ok(ExplorerSnapshot {
            query: QueryResult {
                total,
                rows: page_rows(src, ids)?,
            },
            stats,
            sources,
            codes,
        })
    })
}

// ---------------------------------------------------------------- charts

/// `analysis::compute_series_stream` over the engine's columns.
pub(crate) fn series(src: &Source, pfs: &[PreparedFilter], spec: &SeriesSpec) -> Result<Option<SeriesResult>> {
    analytics_with(src, false, |session| series_of(&scope(session, src, pfs)?, spec))
}

/// Only the requested metric, its admitted sample count and unit warnings cross
/// the engine boundary. Avoid DISTINCT state and ordered sums for other metrics.
/// The first numeric column is a sum for avg; division stays in Rust to preserve
/// the existing arithmetic and the file-order sum used by the line engine.
fn metric_columns(value: Option<&str>, expected: Option<UnitKind>, metric: &str) -> (String, String) {
    if metric == "count" {
        return ("NULL::DOUBLE AS num".into(), "count(*), count(*), 0::BIGINT".into());
    }
    let Some(value) = value else {
        return ("NULL::DOUBLE AS num".into(), "NULL::DOUBLE, 0::BIGINT, 0::BIGINT".into());
    };
    if metric == "distinct" {
        return (
            format!("{value} AS v"),
            "count(DISTINCT CASE WHEN v <> '' THEN v END), count(*) FILTER (WHERE v <> ''), 0::BIGINT".into(),
        );
    }
    let (inner, ok, incompatible) = match expected {
        Some(unit) => (
            format!("li_num({value}) AS num, li_unit({value}) AS u"),
            format!("num IS NOT NULL AND u = {}", unit as i32),
            format!("count(*) FILTER (WHERE num IS NOT NULL AND u <> {})", unit as i32),
        ),
        None => (format!("li_num({value}) AS num"), "num IS NOT NULL".into(), "0::BIGINT".into()),
    };
    let numeric = match metric {
        "sum" | "avg" => format!("sum(CASE WHEN {ok} THEN num END ORDER BY id)"),
        "min" => format!("min(CASE WHEN {ok} THEN num END)"),
        "max" => format!("max(CASE WHEN {ok} THEN num END)"),
        _ => "NULL::DOUBLE".into(),
    };
    (inner, format!("{numeric}, count(*) FILTER (WHERE {ok}), {incompatible}"))
}

struct Metric {
    value: f64,
    n: usize,
    incompatible: usize,
}

/// Reads value (or avg's ordered sum), admitted samples and incompatible units.
fn metric_at(row: &StreamRow<'_>, at: usize, metric: &str) -> Result<Metric> {
    let n = row.get::<i64>(at + 1)? as usize;
    let incompatible = row.get::<i64>(at + 2)? as usize;
    let value = match metric {
        "count" | "distinct" => row.get::<Option<i64>>(at)?.unwrap_or(0) as usize as f64,
        _ => row.get::<Option<f64>>(at)?.unwrap_or(0.0),
    };
    let value = if metric == "avg" {
        if n == 0 { 0.0 } else { value / n as f64 }
    } else { value };
    Ok(Metric { value, n, incompatible })
}

/// The heap root is the worst retained group. Rank tuples use the existing
/// Rust ordering, which SQL collation/NULL/float ordering cannot substitute.
/// Account live keys plus one candidate, releasing evicted keys immediately;
/// DuckDB's grouping state and Arrow fetch chunks are outside this retention budget.
struct SeriesTopK<T: Ord> {
    heap: BinaryHeap<(T, usize)>,
    limit: usize,
    key_bytes: usize,
    base_bytes: usize,
    byte_limit: usize,
    accounted: usize,
    admission: Option<Arc<crate::analysis_runtime::Admitted>>,
}

impl<T: Ord> SeriesTopK<T> {
    fn new(limit: usize) -> Result<Self> {
        let base_bytes = limit.saturating_add(1).saturating_mul(std::mem::size_of::<(T, usize)>());
        let byte_limit = crate::resources::analytics_bytes();
        Self::check_bytes(base_bytes, byte_limit)?;
        let admission = crate::analysis_runtime::current();
        if let Some(admitted) = &admission { admitted.retain_resource_bytes(base_bytes)?; }
        Ok(Self { heap: BinaryHeap::with_capacity(limit), limit, key_bytes: 0, base_bytes, byte_limit, accounted: base_bytes, admission })
    }

    fn push(&mut self, rank: T, bytes: usize) -> Result<()> {
        let peak = self.base_bytes.saturating_add(self.key_bytes).saturating_add(bytes);
        Self::check_bytes(peak, self.byte_limit)?;
        if peak > self.accounted {
            if let Some(admitted) = &self.admission { admitted.retain_resource_bytes(peak - self.accounted)?; }
            self.accounted = peak;
        }
        if self.limit == 0 { return Ok(()); }
        if self.heap.len() == self.limit {
            if rank >= self.heap.peek().expect("nonempty top groups").0 { return Ok(()); }
            let (_, removed) = self.heap.pop().expect("nonempty top groups");
            self.key_bytes -= removed;
        }
        self.key_bytes += bytes;
        self.heap.push((rank, bytes));
        Ok(())
    }

    fn finish(self) -> Vec<T> {
        self.heap.into_sorted_vec().into_iter().map(|(rank, _)| rank).collect()
    }

    fn check_bytes(bytes: usize, limit: usize) -> Result<()> {
        if bytes > limit {
            Err("O resultado analítico excedeu o orçamento de valores (LOGINSIGHT_ANALYTICS_LIMIT_MB). Restrinja os filtros ou reduza os agrupamentos.".into())
        } else { Ok(()) }
    }
}

/// Descending total float order, including NaN payloads and signed zero.
#[derive(Clone, Copy, Debug)]
struct SeriesMetricRank(f64);
impl PartialEq for SeriesMetricRank {
    fn eq(&self, other: &Self) -> bool { self.cmp(other).is_eq() }
}
impl Eq for SeriesMetricRank {}
impl PartialOrd for SeriesMetricRank {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> { Some(self.cmp(other)) }
}
impl Ord for SeriesMetricRank {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering { other.0.total_cmp(&self.0) }
}

/// Own each label once and fill the final numeric buffers by stable series ID.
/// Label storage is independent of bucket count; empty buckets start at zero.
fn time_series_buffers(names: Vec<String>, buckets: usize) -> Vec<SeriesData> {
    names.into_iter().map(|name| SeriesData {
        name,
        samples: vec![0; buckets],
        points: vec![0.0; buckets],
    }).collect()
}

#[cfg(test)]
mod series_retention_tests {
    use super::*;

    #[test]
    fn time_bucket_label_storage_is_independent_of_bucket_count() {
        let buckets = 2001;
        let names: Vec<_> = (0..6).map(|i| format!("{i}{}", "x".repeat((64 << 10) - 1))).collect();
        let pointers: Vec<_> = names.iter().map(|s| s.as_ptr()).collect();
        let label_bytes = names.iter().map(String::capacity).sum::<usize>();
        let series = time_series_buffers(names, buckets);
        assert_eq!(series.iter().map(|s| s.name.as_ptr()).collect::<Vec<_>>(), pointers,
            "transfer the selected strings without cloning their allocation");
        assert_eq!(series.iter().map(|s| s.name.capacity()).sum::<usize>(), label_bytes);
        assert!(series.iter().all(|s| s.points.len() == buckets && s.samples.len() == buckets));
        let numeric_bytes = series.iter().map(|s| s.points.capacity() * std::mem::size_of::<f64>()
            + s.samples.capacity() * std::mem::size_of::<usize>()).sum::<usize>();
        let old_repeated_key_bytes = label_bytes * buckets;
        assert!(old_repeated_key_bytes > (label_bytes + numeric_bytes) * 1000,
            "structural allocation comparison only, not a measured RSS claim");
    }

    #[test]
    fn count_terms_keep_json_key_ties_in_every_input_order() {
        let keys = [None, Some(""), Some("\n"), Some("\""), Some("\\"), Some("a"), Some("é"), Some("日"), Some("null")];
        let original: Vec<_> = keys.into_iter().enumerate().map(|(i, key)| {
            let key = key.map(str::to_string);
            (serde_json::to_string(&key).unwrap(), key, i % 3)
        }).collect();
        for limit in [0, 1, 3, 10, 500] {
            let mut expected = original.clone();
            expected.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
            expected.truncate(limit);
            for offset in 0..original.len() {
                let mut selected = SeriesTopK::new(limit).unwrap();
                for (serialized, key, n) in original.iter().cycle().skip(offset).take(original.len()) {
                    let bytes = serialized.capacity() + key.as_ref().map_or(0, String::capacity);
                    selected.push((Reverse(*n), serialized.clone(), key.clone()), bytes).unwrap();
                }
                let found: Vec<_> = selected.finish().into_iter().map(|(Reverse(n), serialized, key)| (serialized, key, n)).collect();
                assert_eq!(found, expected);
            }
        }
    }

    #[test]
    fn metric_terms_keep_total_float_and_optional_key_ties() {
        let values = [f64::NEG_INFINITY, -1.0, -0.0, 0.0, 1.0, f64::INFINITY,
            f64::from_bits(0x7ff8000000000000), f64::from_bits(0x7ff8000000000001), f64::from_bits(0xfff8000000000000)];
        let original: Vec<_> = values.into_iter().flat_map(|value| [None, Some("".into()), Some("a".into()), Some("日".into())]
            .into_iter().map(move |key| (key, value, 3usize))).collect();
        for limit in [0, 1, 7, 500] {
            let mut expected = original.clone();
            expected.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            expected.truncate(limit);
            for backwards in [false, true] {
                let mut input = original.clone();
                if backwards { input.reverse(); }
                let mut selected = SeriesTopK::new(limit).unwrap();
                for (key, value, n) in input {
                    let bytes = key.as_ref().map_or(0, String::capacity);
                    selected.push((SeriesMetricRank(value), key, n), bytes).unwrap();
                }
                let found: Vec<_> = selected.finish().into_iter().map(|(SeriesMetricRank(value), key, n)| (key, value.to_bits(), n)).collect();
                let expected: Vec<_> = expected.iter().map(|(key, value, n)| (key.clone(), value.to_bits(), *n)).collect();
                assert_eq!(found, expected);
            }
        }
    }

    #[test]
    fn split_names_keep_raw_string_ties() {
        let mut original: Vec<_> = ["\n", "\"", "\\", "a", "b", "c", "d", "é", "日"]
            .into_iter().map(|key| (key.to_string(), 1i64)).collect();
        original.push(("popular".into(), 5));
        let mut selected = SeriesTopK::new(6).unwrap();
        for (key, n) in original.iter().rev() {
            selected.push((Reverse(*n), key.clone()), key.capacity()).unwrap();
        }
        original.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        original.truncate(6);
        assert_eq!(selected.finish().into_iter().map(|(Reverse(n), key)| (key, n)).collect::<Vec<_>>(), original);
    }

    #[test]
    fn retained_bytes_are_released_on_eviction_and_discard() {
        crate::resources::with_analytics_limit(4096, || {
            let mut selected = SeriesTopK::new(6).unwrap();
            for i in 0..10_000usize {
                let key = format!("{i:05}{}", "x".repeat(200));
                let bytes = key.capacity();
                selected.push((Reverse(i), key), bytes).unwrap();
                assert!(selected.heap.len() <= 6);
                assert_eq!(selected.key_bytes, selected.heap.iter().map(|(_, bytes)| bytes).sum::<usize>());
                assert!(selected.base_bytes + selected.key_bytes <= 4096);
            }
            assert_eq!(selected.finish().iter().map(|(Reverse(n), _)| *n).collect::<Vec<_>>(), (9994..10_000).rev().collect::<Vec<_>>());
        });
    }

    #[test]
    fn candidate_bytes_are_checked_even_when_it_would_be_discarded() {
        crate::resources::with_analytics_limit(1024, || {
            let mut selected = SeriesTopK::new(1).unwrap();
            selected.push((Reverse(2), "best".to_string()), 4).unwrap();
            let key = "x".repeat(1024);
            assert!(selected.push((Reverse(1), key), 1024).unwrap_err().contains("LOGINSIGHT_ANALYTICS_LIMIT_MB"));
            assert_eq!(selected.finish(), vec![(Reverse(2), "best".to_string())]);
        });
    }
}

fn series_of(scope: &Scope, spec: &SeriesSpec) -> Result<SeriesResult> {
    let limit = spec.limit.unwrap_or(10).min(500);
    let field = spec.field.as_deref();
    let mut names = false;
    if spec.chart == "terms" && spec.metric == "count" {
        let key_column = field.unwrap_or("level");
        let (key, time) = key_of(scope, key_column, false, &mut names)?;
        let from = scope.from(names);
        let mut counted = SeriesTopK::new(limit)?;
        stream_rows(
            scope.session,
            &format!("SELECT k, count(*) FROM (SELECT {key} AS k FROM {from} WHERE {}) GROUP BY k", scope.cond),
            |r| {
                let key = stream_key(&r, 0, time)?;
                let n = r.get::<i64>(1)? as usize;
                let serialized = serde_json::to_string(&key).map_err(err)?;
                let bytes = serialized.capacity().saturating_add(key.as_ref().map_or(0, String::capacity));
                counted.push((Reverse(n), serialized, key), bytes)
            },
        )?;
        let counted = counted.finish();
        return Ok(SeriesResult {
            kind: "terms".into(),
            unit: "number".into(),
            interval_ms: 0,
            x: counted
                .iter()
                .map(|(_, _, value)| Value::from(value.clone().unwrap_or_else(|| "(vazio)".into())))
                .collect(),
            x_values: counted.iter().map(|(_, _, value)| value.clone()).collect(),
            series: vec![SeriesData {
                name: key_column.into(),
                samples: counted.iter().map(|(Reverse(n), _, _)| *n).collect(),
                points: counted.iter().map(|(Reverse(n), _, _)| *n as f64).collect(),
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
            let mut counted = SeriesTopK::new(6)?;
            stream_rows(
                scope.session,
                &format!(
                    "SELECT s, count(*) FROM (SELECT {split} AS s FROM {from} WHERE {}) WHERE s <> '' GROUP BY s",
                    scope.cond
                ),
                |r| {
                    let key = r.get::<String>(0)?;
                    let bytes = key.capacity();
                    counted.push((Reverse(r.get::<i64>(1)?), key), bytes)
                },
            )?;
            counted.finish().into_iter().map(|(_, key)| key).collect()
        }
        None => Vec::new(),
    };
    let has_splits = !splits.is_empty();
    let split_names: Vec<String> = if !has_splits {
        vec![spec.field.clone().unwrap_or_else(|| "eventos".into())]
    } else {
        splits
    };
    let (metric_inner, metric_outer) = metric_columns(value.as_deref(), expected, &spec.metric);

    if spec.chart == "terms" {
        let key_column = spec.field.clone().unwrap_or_else(|| "level".into());
        let (key, time) = key_of(scope, &key_column, false, &mut names)?;
        let from = scope.from(names);
        let mut items = SeriesTopK::new(limit)?;
        let mut incompatible = 0;
        stream_rows(
            scope.session,
            &format!(
                "SELECT k, {metric_outer} FROM (SELECT id, {key} AS k, {metric_inner} FROM {from} WHERE {}) GROUP BY k",
                scope.cond
            ),
            |r| {
                let key = stream_key(&r, 0, time)?;
                let metric = metric_at(&r, 1, &spec.metric)?;
                // Unit warnings cover all groups, including those below Top N.
                incompatible += metric.incompatible;
                let bytes = key.as_ref().map_or(0, String::capacity);
                items.push((SeriesMetricRank(metric.value), key, metric.n), bytes)
            },
        )?;
        let items = items.finish();
        return Ok(SeriesResult {
            kind: "terms".into(),
            unit,
            interval_ms: 0,
            x: items
                .iter()
                .map(|(_, k, _)| Value::from(k.clone().unwrap_or_else(|| "(vazio)".into())))
                .collect(),
            x_values: items.iter().map(|(_, key, _)| key.clone()).collect(),
            series: vec![SeriesData {
                name: split_names[0].clone(),
                samples: items.iter().map(|(_, _, n)| *n).collect(),
                points: items.into_iter().map(|(SeriesMetricRank(value), _, _)| value).collect(),
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
    let series_sql = match &spec.split {
        Some(column) if has_splits => {
            let split = text_of(scope, column, false, &mut names)?;
            let mut cases = format!("CASE COALESCE({split}, '')");
            for (index, name) in split_names.iter().enumerate() {
                cases.push_str(&format!(" WHEN {} THEN {index}", lit(name)));
            }
            cases.push_str(" ELSE -1 END");
            cases
        }
        Some(_) => "-1".into(),
        None => "0".into(),
    };
    let from = scope.from(names);
    let mut series = time_series_buffers(split_names, n_buckets);
    let mut incompatible = 0;
    stream_rows(
        scope.session,
        &format!(
            "SELECT b, series_id, {metric_outer} FROM (SELECT id, (ts - ({tmin})) // {interval} AS b, {series_sql} AS series_id, \
             {metric_inner} FROM {from} WHERE ({}) AND ts IS NOT NULL) WHERE series_id >= 0 GROUP BY b, series_id",
            scope.cond
        ),
        |r| {
            let b = r.get::<i64>(0)?;
            let series_id = r.get::<i64>(1)?;
            let metric = metric_at(&r, 2, &spec.metric)?;
            if b < 0 || b as usize >= n_buckets { return Ok(()); }
            incompatible += metric.incompatible;
            if let Ok(index) = usize::try_from(series_id) {
                if let Some(series) = series.get_mut(index) {
                    series.samples[b as usize] = metric.n;
                    series.points[b as usize] = metric.value;
                }
            }
            Ok(())
        },
    )?;
    Ok(SeriesResult {
        kind: "time".into(),
        unit,
        interval_ms: interval,
        x: (0..n_buckets)
            .map(|b| Value::from(tmin.saturating_add((b as i64).saturating_mul(interval))))
            .collect(),
        x_values: vec![],
        series,
        incompatible_units: incompatible,
    })
}

// ---------------------------------------------------------------- overview

/// `insights::overview` over the engine's columns.
pub(crate) fn overview(src: &Source, pfs: &[PreparedFilter]) -> Result<Option<Overview>> {
    analytics_with(src, false, |session| overview_of(&scope(session, src, pfs)?, src))
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
    let sql = format!("SELECT {columns} FROM {from} WHERE ({}) AND ({any}) ORDER BY id", scope.cond);
    let mut values: Vec<Option<String>> = vec![None; present.len()];
    stream_rows(scope.session, &sql, |row| {
        crate::operations::check()?;
        for (i, slot) in values.iter_mut().enumerate() {
            *slot = row.get(i)?;
        }
        sample.push(|k| present.iter().position(|(f, _)| *f == k).and_then(|i| values[i].clone()));
        Ok(())
    })?;
    Ok(sample)
}

// ---------------------------------------------------------------- comparison

/// `insights::compare` over the engine's columns.
pub(crate) fn compare(src: &Source, pfs: &[PreparedFilter], before: &Period, after: &Period) -> Result<Option<Comparison>> {
    analytics_with(src, false, |session| {
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
) -> Result<Option<crate::analysis::PivotResult>> {
    analytics_with(src, false, |session| {
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
        light_events(&scope, &columns, |events| crate::analysis::pivot_stream(events, spec))?
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
    light_events_bounded(
        scope,
        columns,
        8192,
        crate::resources::batch_bytes(),
        &LightStreamWork::default(),
        consume,
    )
}

fn light_events_bounded<T>(
    scope: &Scope,
    columns: &[String],
    batch_rows: usize,
    batch_bytes: usize,
    work: &LightStreamWork,
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
    transfer_events(
        batch_rows,
        batch_bytes,
        work,
        |send| {
            stream_rows(scope.session, &sql, |row| {
                crate::operations::check()?;
                let mut ev = Event::empty();
                ev.id = row.get::<i64>(0)? as usize;
                ev.timestamp = row.get(1)?;
                let mut column = 2;
                for slot in &slots {
                    if matches!(slot, Slot::Timestamp | Slot::Id) {
                        continue;
                    }
                    let value: Option<String> = row.get(column)?;
                    column += 1;
                    if let Slot::Field(name) = slot {
                        if let Some(value) = value {
                            ev.fields.insert(name.clone(), Value::String(value));
                        }
                        continue;
                    }
                    let text = value.unwrap_or_default();
                    match slot {
                        Slot::Source => ev.source = text,
                        Slot::Level => ev.level = text,
                        Slot::Code => ev.code = text,
                        Slot::Message => ev.message = text,
                        Slot::Name => ev.name = text,
                        Slot::Description => ev.description = text,
                        Slot::EventRef => ev.event_ref = text,
                        Slot::Timestamp | Slot::Id | Slot::Field(_) => {}
                    }
                }
                let size = crate::query::event_payload_bytes(&ev);
                send(ev, size)
            })
        },
        consume,
    )
}

/// Byte-bounded transfer used by the light-event SQL reader. This helper is
/// independent of DuckDB so its lifetime/backpressure rules can be tested
/// without an application-wide build.
#[derive(Default)]
struct LightStreamWork {
    rows: std::sync::atomic::AtomicUsize,
    max_batch_rows: std::sync::atomic::AtomicUsize,
    max_batch_bytes: std::sync::atomic::AtomicUsize,
}
impl LightStreamWork {
    fn batch(&self, rows: usize, bytes: usize) {
        self.rows.fetch_add(rows, Ordering::Relaxed);
        self.max_batch_rows.fetch_max(rows, Ordering::Relaxed);
        self.max_batch_bytes.fetch_max(bytes, Ordering::Relaxed);
    }
}

/// One queued batch, one consumer batch and one producer batch. The event
/// which triggers a byte-boundary flush may also be live. As with the source
/// visitor, one oversized event travels alone; values are never truncated.
fn transfer_events<E: Send, T>(
    batch_rows: usize,
    batch_bytes: usize,
    work: &LightStreamWork,
    produce: impl FnOnce(&mut dyn FnMut(E, usize) -> Result<()>) -> Result<()> + Send,
    consume: impl FnOnce(&mut dyn Iterator<Item = E>) -> T,
) -> Result<T> {
    if crate::global_scheduler::current().is_none() {
        return crate::global_scheduler::run(None, crate::global_scheduler::Priority::Normal, &crate::operations::cancelled,
            || transfer_events(batch_rows, batch_bytes, work, produce, consume))?;
    }
    let (sender, receiver) = std::sync::mpsc::sync_channel::<Vec<E>>(1);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    // Direct library callers may have no outer operation. Give the child a
    // generation too, so the SQL interrupt watcher observes its private stop.
    let token = if crate::operations::current_generation().is_none()
        && crate::operations::current_id().is_none()
    {
        crate::operations::run(
            crate::operations::generation(),
            crate::operations::current_token,
        )?
    } else {
        crate::operations::current_token()
    }
    .with_stop(Arc::clone(&stop));
    struct StopProducer(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for StopProducer {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }
    std::thread::scope(|threads| {
        let producer = std::thread::Builder::new()
            .name("loginsight-pivot-stream".into())
            .spawn_scoped(threads, move || -> Result<()> {
                let result = crate::operations::run_with_token(token, || {
                    let mut batch = Vec::new();
                    let mut bytes = 0usize;
                    produce(&mut |event, size| {
                        crate::operations::check()?;
                        if !batch.is_empty()
                            && (batch.len() >= batch_rows.max(1)
                                || bytes.saturating_add(size) > batch_bytes)
                        {
                            work.batch(batch.len(), bytes);
                            if crate::global_scheduler::blocking(|| sender.send(std::mem::take(&mut batch))).is_err() {
                                worker_stop.store(true, Ordering::Relaxed);
                                return Err("Operação cancelada.".into());
                            }
                            bytes = 0;
                        }
                        bytes = bytes.saturating_add(size);
                        batch.push(event);
                        Ok(())
                    })?;
                    if !batch.is_empty() {
                        work.batch(batch.len(), bytes);
                        let _ = crate::global_scheduler::blocking(|| sender.send(batch));
                    }
                    Ok(())
                })
                .and_then(|result| result);
                // Decide while the sender is still owned here. A real fetch error
                // precedes channel EOF and is retained; an intentional early drop
                // interrupts only this child and does not fail the parent query.
                if worker_stop.load(Ordering::Relaxed) {
                    Ok(())
                } else {
                    result
                }
            })
            .map_err(|error| error.to_string())?;
        let mut events = std::iter::from_fn(move || crate::global_scheduler::blocking(|| receiver.recv().ok())).flatten();
        let stop_guard = StopProducer(stop);
        // A consumer panic must close the channel and hand off its lane
        // before scoped threads are joined during unwinding.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| consume(&mut events)));
        drop(stop_guard);
        drop(events);
        crate::global_scheduler::blocking(|| producer.join())
            .map_err(|_| "Falha ao ler o motor de consultas.".to_string())??;
        crate::operations::check()?;
        match result { Ok(value) => Ok(value), Err(panic) => std::panic::resume_unwind(panic) }
    })
}

#[cfg(test)]
mod light_transfer_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn single_slot_stream_handles_full_channel_early_return_and_consumer_panic() {
        crate::global_scheduler::with_limit(1, || {
            let work = LightStreamWork::default();
            let values = transfer_events(2, 32, &work, |send| {
                for n in 0..100 { send(n, 8)?; }
                Ok(())
            }, |events| events.collect::<Vec<_>>()).unwrap();
            assert_eq!(values, (0..100).collect::<Vec<_>>());
            let first = transfer_events(2, 32, &work, |send| {
                for n in 0..100 { send(n, 8)?; }
                Ok(())
            }, |events| events.next()).unwrap();
            assert_eq!(first, Some(0));
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                transfer_events(2, 32, &work, |send| {
                    for n in 0..100 { send(n, 8)?; }
                    Ok(())
                }, |events| { assert_eq!(events.next(), Some(0)); panic!("controlled stream consumer panic"); })
            }));
            assert!(panic.is_err());
            assert_eq!(transfer_events(2, 32, &work, |send| send(7, 8), |events| events.sum::<i32>()).unwrap(), 7);
        });
    }

    #[test]
    fn light_transfer_preserves_order_with_row_and_byte_bounds() {
        let work = LightStreamWork::default();
        let found = transfer_events(
            7,
            512,
            &work,
            |send| {
                for id in 0..137 {
                    let value = vec![id as u8; 128 + id % 5];
                    let bytes = value.len();
                    send((id, value), bytes)?;
                }
                Ok(())
            },
            |events| {
                events
                    .map(|(id, value)| {
                        assert_eq!(value, vec![id as u8; 128 + id % 5]);
                        id
                    })
                    .collect::<Vec<_>>()
            },
        )
        .unwrap();
        assert_eq!(found, (0..137).collect::<Vec<_>>());
        assert_eq!(work.rows.load(Ordering::Relaxed), 137);
        assert!(work.max_batch_rows.load(Ordering::Relaxed) <= 7);
        assert!(work.max_batch_bytes.load(Ordering::Relaxed) <= 512);
    }

    #[test]
    fn light_transfer_keeps_one_oversized_value_intact_in_its_own_batch() {
        let work = LightStreamWork::default();
        let found = transfer_events(
            8,
            128,
            &work,
            |send| {
                for size in [10, 2048, 10] {
                    send(vec![42u8; size], size)?;
                }
                Ok(())
            },
            |events| {
                events
                    .map(|value| {
                        assert!(value.iter().all(|byte| *byte == 42));
                        value.len()
                    })
                    .collect::<Vec<_>>()
            },
        )
        .unwrap();
        assert_eq!(found, vec![10, 2048, 10]);
        assert_eq!(work.max_batch_rows.load(Ordering::Relaxed), 1);
        assert_eq!(work.max_batch_bytes.load(Ordering::Relaxed), 2048);
    }

    #[test]
    fn light_transfer_early_drop_bounds_production_without_cancelling_parent() {
        let work = LightStreamWork::default();
        let produced = AtomicUsize::new(0);
        let token = crate::operations::token(None).unwrap();
        let parent = token.clone();
        let found = crate::operations::run_with_token(token, || {
            transfer_events(
                4,
                128,
                &work,
                |send| {
                    for id in 0..10_000 {
                        produced.fetch_add(1, Ordering::Relaxed);
                        send(id, 8)?;
                    }
                    Ok(())
                },
                |events| events.take(1).collect::<Vec<_>>(),
            )
        })
        .unwrap()
        .unwrap();
        assert_eq!(found, vec![0]);
        assert!(
            produced.load(Ordering::Relaxed) <= 13,
            "at most three batches and the boundary event"
        );
        assert!(work.rows.load(Ordering::Relaxed) <= 12);
        assert!(!parent.cancelled());
    }

    #[test]
    fn light_transfer_propagates_producer_failure_instead_of_partial_success() {
        let work = LightStreamWork::default();
        let seen = AtomicUsize::new(0);
        let error = transfer_events(
            2,
            128,
            &work,
            |send| {
                for id in 0..20 {
                    send(id, 8)?;
                }
                Err("late reader failure".into())
            },
            |events| {
                events.for_each(|_| {
                    seen.fetch_add(1, Ordering::Relaxed);
                })
            },
        )
        .unwrap_err();
        assert!(error.contains("late reader failure"));
        assert!(seen.load(Ordering::Relaxed) > 0);
        assert_eq!(
            transfer_events(
                1,
                128,
                &work,
                |send| send(42, 8),
                |events| events.collect::<Vec<_>>()
            )
            .unwrap(),
            vec![42]
        );
    }

    #[test]
    fn light_transfer_inherits_named_cancellation_and_progress_identity() {
        let id = format!("light-transfer-{}", uuid::Uuid::new_v4());
        let token = crate::operations::token(Some(id.clone())).unwrap();
        let unrelated = crate::operations::token(None).unwrap();
        let progress = Arc::new(std::sync::Mutex::new(Vec::new()));
        let capture = Arc::clone(&progress);
        let reporter: crate::operations::Reporter =
            Arc::new(move |event| capture.lock().unwrap().push(event));
        let work = LightStreamWork::default();
        let result = crate::operations::run_with_token(token, || {
            crate::operations::with_reporter(reporter, || {
                transfer_events(
                    2,
                    128,
                    &work,
                    |send| {
                        crate::operations::progress("pivot-values", "Lendo valores", 0, 0, 0);
                        for id in 0..1000 {
                            send(id, 8)?;
                        }
                        Ok(())
                    },
                    |events| {
                        assert_eq!(events.next(), Some(0));
                        assert!(crate::operations::cancel_id(&id));
                    },
                )
            })
        });
        assert!(result.is_err());
        assert!(!unrelated.cancelled());
        let progress = progress.lock().unwrap();
        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].operation_id.as_deref(), Some(id.as_str()));
        assert_eq!(progress[0].phase_id, "pivot-values");
    }

    #[test]
    fn light_transfer_consumer_unwind_stops_the_child_without_hanging() {
        let token = crate::operations::token(None).unwrap();
        let parent = token.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::operations::run_with_token(token, || {
                transfer_events::<usize, ()>(
                    2,
                    128,
                    &LightStreamWork::default(),
                    |send| {
                        for id in 0..10_000 {
                            send(id, 8)?;
                        }
                        Ok(())
                    },
                    |events| {
                        assert_eq!(events.next(), Some(0));
                        panic!("controlled consumer panic");
                    },
                )
            })
        }));
        assert!(result.is_err());
        assert!(!parent.cancelled());
    }
}

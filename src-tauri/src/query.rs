use crate::model::CodesConfig;
use crate::model::{label_class, Event, LineMeta, LV_OTHER};
use crate::sources::{event_at, line_bytes, CompiledDerived, FileIndex};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct Filter {
    pub column: String,
    pub op: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub value2: Option<String>,
}

/// Filtro com regex pré-compilada; entradas inválidas são rejeitadas na fronteira.
/// `needle_lower`, `num` e `num2` são pré-computados uma vez por consulta
/// em vez de por evento/linha.
pub struct PreparedFilter {
    pub f: Filter,
    regex: Option<regex::Regex>,
    needle_lower: String,
    num: Option<f64>,
    num2: Option<f64>,
    threat: Option<crate::threats::RuleMatcher>,
    /// Compiled search expression (`op = "query"`).
    pub(crate) expr: Option<crate::querylang::Expr>,
    /// Lowercased values for `in` / `not_in`.
    set: Option<std::collections::HashSet<String>>,
    /// Networks for `cidr` / `not_cidr`.
    nets: Vec<crate::querylang::IpNet>,
    /// Detection rule reproduced as evidence filter (`op = "detection"`).
    detection: Option<(std::sync::Arc<crate::detections::RuleSet>, usize)>,
}

impl PreparedFilter {
    /// Parsed numeric bounds, shared with the SQL compiler so native range
    /// predicates use exactly the same timestamp/unit parsing as the reader.
    pub(crate) fn numeric_bounds(&self) -> (Option<f64>, Option<f64>) {
        (self.num, self.num2)
    }
}

/// Values of an `in` filter: one per line (commas also separate).
pub fn list_values(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(['\n', '\r', ','])
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

pub fn prepare(filters: &[Filter]) -> Vec<PreparedFilter> {
    prepare_with_threat_catalog(filters, None)
}

pub(crate) fn prepare_with_threat_catalog(
    filters: &[Filter],
    catalog: Option<&std::sync::Arc<crate::threats::CompiledCatalog>>,
) -> Vec<PreparedFilter> {
    filters
        .iter()
        .map(|f| {
            let is_re = f.op == "regex";
            let is_query = f.op == "query";
            PreparedFilter {
                regex: is_re.then(|| crate::query_regex::compile(&f.value, crate::query_regex::ORDINARY).ok()).flatten(),
                needle_lower: if is_query { String::new() } else { f.value.to_lowercase() },
                num: value_as_num(&f.column, &f.value),
                num2: value_as_num(&f.column, f.value2.as_deref().unwrap_or("")),
                threat: if f.op == "threat_rule" {
                    crate::threats::matcher(&f.value, catalog.cloned()).ok()
                } else {
                    None
                },
                expr: is_query
                    .then(|| {
                        crate::querylang::compile_with(
                            &f.value,
                            &crate::querylang::Options { threats: catalog, detections: true },
                        )
                        .ok()
                    })
                    .flatten(),
                set: if f.op == "in_exact" { Some(f.value.lines().filter(|v| !v.is_empty()).map(str::to_string).collect()) }
                    else { matches!(f.op.as_str(), "in" | "not_in").then(|| list_values(&f.value).map(str::to_lowercase).collect()) },
                nets: if matches!(f.op.as_str(), "cidr" | "not_cidr") {
                    list_values(&f.value)
                        .flat_map(|v| v.split_whitespace())
                        .filter_map(crate::querylang::IpNet::parse)
                        .collect()
                } else {
                    Vec::new()
                },
                detection: (f.op == "detection")
                    .then(|| {
                        let set = crate::detections::ruleset().ok()?;
                        let index = set.rules.iter().position(|r| r.def.id == f.value)?;
                        Some((set, index))
                    })
                    .flatten(),
                f: f.clone(),
            }
        })
        .collect()
}

#[derive(Serialize)]
pub struct QueryResult {
    pub total: usize,
    pub rows: Vec<Event>,
}

/// Interactive pages do not wait for an exact global count. The exact APIs
/// remain available to analytics, exports and MCP callers.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryPage {
    pub rows: Vec<Event>,
    pub total: Option<usize>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub engine: String,
    pub warning: Option<String>,
}

impl QueryPage {
    pub(crate) fn from_exact(result: QueryResult, offset: usize, engine: &str, warning: Option<String>) -> Self {
        let has_more = offset.saturating_add(result.rows.len()) < result.total;
        Self {
            rows: result.rows,
            total: Some(result.total),
            has_more,
            next_cursor: None,
            engine: engine.into(),
            warning,
        }
    }
}

/// Exact page membership, shared by full Event and opt-in projected responses.
/// IDs are memory positions on the memory route and indexed IDs otherwise.
pub(crate) struct SelectedPage {
    pub ids: Vec<usize>,
    pub total: Option<usize>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub engine: String,
    pub warning: Option<String>,
}
impl SelectedPage {
    pub(crate) fn into_full(self, rows: Vec<Event>) -> QueryPage {
        QueryPage { rows, total: self.total, has_more: self.has_more,
            next_cursor: self.next_cursor, engine: self.engine, warning: self.warning }
    }
    fn exact(ids: Vec<usize>, total: usize, offset: usize, engine: &str, warning: Option<String>) -> Self {
        Self { has_more: offset.saturating_add(ids.len()) < total, ids,
            total: Some(total), next_cursor: None, engine: engine.into(), warning }
    }
}

#[derive(Serialize, Default)]
pub struct AggResult {
    pub columns: Vec<String>,
    pub rows: Vec<Map<String, Value>>,
    /// Exact grouping values, parallel to rows; None is an empty/missing value.
    pub group_values: Vec<Option<String>>,
    pub incompatible_units: Vec<usize>,
    pub value_units: Vec<String>,
    /// Groups left out beyond [`MAX_GROUPS`] (the smallest by the first measure).
    pub omitted_groups: usize,
    /// Records counted in the omitted groups, when a count is among the measures.
    pub omitted_records: u64,
    /// Explicit bounded-work failure. Never publish a partial aggregate as exact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl AggResult {
    pub(crate) fn failure(error: String) -> Self { Self { error: Some(error), ..Self::default() } }
    pub(crate) fn budget_error() -> Self {
        Self { error: Some("Agrupamento excede o orçamento de grupos. Reduza o período/filtros, escolha um campo menos distinto ou use uma contagem simples.".into()), ..Self::default() }
    }
}

fn group_budget(specs: &[AggSpec]) -> usize {
    if !specs.is_empty() && specs.iter().all(|spec| spec.func == "count") { MAX_GROUPS * 2 } else { MAX_GROUPS }
}

/// Groups returned at most. Values with millions of distinct groups (ids,
/// messages) would otherwise produce payloads the interface cannot render.
pub(crate) const MAX_GROUPS: usize = 50_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub buckets: Vec<(i64, i64)>,
    pub bucket_ms: i64,
    pub levels: Vec<(String, i64)>,
}

/// Recorte completo do Explorador. A seleção de linhas é calculada uma vez e
/// reutilizada para tabela, histograma e facetas no mesmo ciclo de leitura.
#[derive(Serialize)]
pub struct ExplorerSnapshot {
    pub query: QueryResult,
    pub stats: Stats,
    pub sources: AggResult,
    pub codes: AggResult,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
pub struct AggSpec {
    pub func: String,
    pub column: String,
    pub alias: String,
}

/// Interpreta o valor digitado no filtro como número. Para a coluna
/// `timestamp`, aceita epoch em ms ou texto ISO ("2024-01-01 10:30").
fn value_as_num(column: &str, s: &str) -> Option<f64> {
    let s = s.trim();
    if let Ok(n) = s.parse::<f64>() {
        return Some(n);
    }
    if column == "timestamp" {
        return crate::sources::parse_timestamp(s).map(|ms| ms as f64);
    }
    // aceita valores com unidade ("100 MB", "2s") nos filtros >, < e entre
    crate::analysis::parse_num_unit(s).map(|(n, _)| n)
}

pub fn matches(ev: &Event, pf: &PreparedFilter) -> bool {
    matches_in_domain(ev, pf, false)
}

/// Line metadata reserves zero for an absent timestamp. Only indexed residual
/// verification inherits that sentinel; a standalone Event's Some(0) is a
/// present timestamp and must remain distinct from None in Cases/memory views.
pub(crate) fn matches_indexed(ev: &Event, pf: &PreparedFilter) -> bool {
    matches_in_domain(ev, pf, true)
}

fn matches_in_domain(ev: &Event, pf: &PreparedFilter, indexed: bool) -> bool {
    let f = &pf.f;
    let op = f.op.as_str();
    match op {
        "threat_rule" => {
            return f.column == "_all"
                && pf
                    .threat
                    .as_ref()
                    .is_some_and(|matcher| matcher.matches(ev))
        }
        // An invalid expression was rejected by validation; never match silently.
        "query" => return pf.expr.as_ref().is_some_and(|expr| {
            if indexed { expr.matches_indexed(ev) } else { expr.matches(ev) }
        }),
        "detection" => {
            return pf
                .detection
                .as_ref()
                .is_some_and(|(set, index)| set.rules[*index].matches(ev))
        }
        "pattern" => return crate::insights::pattern_of(&ev.message) == f.value,
        _ => {}
    }
    if f.column == "_all" && !reads_all_as_column(op) {
        let text = format!("{}\n{}", ev.message, ev.raw);
        let needle = pf.needle_lower.as_bytes();
        return match op {
            "regex" => pf.regex.as_ref().is_some_and(|re| re.is_match(&text)),
            "contains" => ci_contains_bytes(text.as_bytes(), needle),
            "not_contains" => !ci_contains_bytes(text.as_bytes(), needle),
            _ => false,
        };
    }
    if is_numeric_op(op) {
        let number = ev.col_num(&f.column);
        return number_matches(pf, number.filter(|value| !indexed || f.column != "timestamp" || *value != 0.0));
    }
    value_matches(pf, ev.col_ref(&f.column).as_deref())
}

/// List and network operators read a column literally named `_all`.
pub(crate) fn reads_all_as_column(op: &str) -> bool {
    matches!(op, "in" | "not_in" | "in_exact" | "cidr" | "not_cidr")
}

pub(crate) fn is_numeric_op(op: &str) -> bool {
    matches!(op, "gt" | "gte" | "lt" | "lte" | "between")
}

/// Numeric operators applied to the column's number (`None` when absent).
pub(crate) fn number_matches(pf: &PreparedFilter, a: Option<f64>) -> bool {
    let Some(a) = a else { return false };
    match pf.f.op.as_str() {
        "between" => match (pf.num, pf.num2) {
            (Some(lo), Some(hi)) => a >= lo && a <= hi,
            _ => false,
        },
        op => {
            let Some(b) = pf.num else { return false };
            match op {
                "gt" => a > b,
                "gte" => a >= b,
                "lt" => a < b,
                _ => a <= b,
            }
        }
    }
}

#[cfg(test)]
mod timestamp_domain_tests {
    use super::*;

    #[test]
    fn preserved_epoch_zero_is_present_but_indexed_numeric_verification_uses_its_sentinel() {
        for timestamp in [None, Some(-1), Some(0), Some(1)] {
            let mut event = Event::empty(); event.timestamp = timestamp;
            let mut indexed_reference = event.clone();
            indexed_reference.timestamp = timestamp.filter(|value| *value != 0);
            for op in ["gt", "gte", "lt", "lte", "between"] {
                let filter = Filter { column: "timestamp".into(), op: op.into(), value: "0".into(), value2: Some("1".into()) };
                let prepared = prepare(&[filter]);
                assert_eq!(matches_indexed(&event, &prepared[0]), matches(&indexed_reference, &prepared[0]), "{timestamp:?} {op}");
                if timestamp == Some(0) && matches!(op, "gte" | "lte" | "between") {
                    assert!(matches(&event, &prepared[0]), "a preserved epoch value remains queryable");
                    assert!(!matches_indexed(&event, &prepared[0]));
                }
            }
            for source in ["timestamp>=0", "NOT timestamp>=0", "NOT (NOT timestamp>=0)",
                "timestamp:0..1 OR code:missing", "timestamp>=0 AND NOT timestamp>1"] {
                let expression = crate::querylang::compile_rule(source).unwrap();
                assert_eq!(expression.matches_indexed(&event), expression.matches(&indexed_reference), "{timestamp:?} {source}");
                let query = prepare(&[Filter { column: "_all".into(), op: "query".into(), value: source.into(), value2: None }]);
                assert_eq!(matches(&event, &query[0]), expression.matches(&event));
                assert_eq!(matches_indexed(&event, &query[0]), expression.matches_indexed(&event));
            }
        }
        let object = serde_json::json!({"timestamp":0});
        assert!(crate::querylang::compile_rule("timestamp>=0").unwrap().matches_object(object.as_object().unwrap()));
        let zero = Event::empty();
        let id = prepare(&[Filter { column: "id".into(), op: "gte".into(), value: "0".into(), value2: None }]);
        assert!(matches(&zero, &id[0]) && matches_indexed(&zero, &id[0]));
    }
}

/// Text operators applied to the column's value (`None` when absent).
pub(crate) fn value_matches(pf: &PreparedFilter, value: Option<&str>) -> bool {
    let f = &pf.f;
    let needle = pf.needle_lower.as_str();
    if let Some(set) = &pf.set {
        if f.op == "in_exact" {
            return value.is_some_and(|v| set.contains(v));
        }
        let hit = value.is_some_and(|v| set.contains(&v.trim().to_lowercase()));
        return (f.op == "in") == hit;
    }
    match f.op.as_str() {
        "cidr" | "not_cidr" => {
            let hit = value
                .and_then(crate::entities::parse_ip)
                .is_some_and(|ip| pf.nets.iter().any(|n| n.contains(ip)));
            (f.op == "cidr") == hit
        }
        // Regex inválida é rejeitada na validação; aqui nunca casa.
        "regex" => pf
            .regex
            .as_ref()
            .is_some_and(|re| re.is_match(value.unwrap_or(""))),
        "contains" => value.is_some_and(|s| ci_contains_bytes(s.as_bytes(), needle.as_bytes())),
        "not_contains" => value.is_none_or(|s| !ci_contains_bytes(s.as_bytes(), needle.as_bytes())),
        "equals" => value.is_some_and(|s| s.eq_ignore_ascii_case(f.value.trim())),
        "not_equals" => value.is_none_or(|s| !s.eq_ignore_ascii_case(f.value.trim())),
        "equals_exact" => value.is_some_and(|s| s == f.value),
        "not_equals_exact" => value.is_none_or(|s| s != f.value),
        "starts_with" => value.is_some_and(|s| {
            let (hay, ndl) = (s.as_bytes(), needle.as_bytes());
            hay.len() >= ndl.len() && hay[..ndl.len()].eq_ignore_ascii_case(ndl)
        }),
        "empty" => value.is_none_or(|s| s.trim().is_empty()),
        "not_empty" => value.is_some_and(|s| !s.trim().is_empty()),
        op if is_numeric_op(op) => number_matches(pf, value.and_then(crate::model::text_number)),
        _ => false,
    }
}

/// Verifica um único filtro contra um evento (condições de campos derivados).
pub fn matches_filter(ev: &Event, filter: &Filter) -> bool {
    let prepared = prepare(std::slice::from_ref(filter));
    matches(ev, &prepared[0])
}

pub fn filtered_indices(events: &[Event], filters: &[Filter]) -> Vec<usize> {
    if filters.is_empty() {
        return (0..events.len()).collect();
    }
    let pfs = prepare(filters);
    events
        .iter()
        .enumerate()
        .take_while(|_| !crate::operations::cancelled())
        .filter(|(_, ev)| pfs.iter().all(|pf| matches(ev, pf)))
        .map(|(i, _)| i)
        .collect()
}

pub(crate) fn count_memory(events: &[Event], filters: &[Filter]) -> usize {
    let pfs = prepare(filters);
    events.iter().take_while(|_| !crate::operations::cancelled()).filter(|event| pfs.iter().all(|pf| matches(event, pf))).count()
}

pub(crate) fn count_lines(idx: &FileIndex, filters: &[Filter], codes: &CodesConfig, system: &CodesConfig, derived: &[CompiledDerived]) -> Result<usize, String> {
    if filters.is_empty() { return crate::analysis_runtime::visible_total(idx); }
    let mut count = 0usize;
    scan_indexed(idx, &prepare(filters), codes, system, derived, |_, _, _| (), |()| count += 1)?;
    Ok(count)
}

pub fn sort_indices(events: &[Event], indices: &mut [usize], column: &str, desc: bool) {
    // Pré-computa a chave (número opcional + texto em minúsculas) uma vez por
    // linha; o comparador anterior realocava as strings a cada comparação.
    let mut keyed: Vec<((Option<f64>, String), usize)> = indices
        .iter()
        .take_while(|_| !crate::operations::cancelled())
        .map(|&i| {
            let ev = &events[i];
            let num = ev.col_num(column);
            let text = ev.col_ref(column).unwrap_or_default().to_lowercase();
            ((num, text), i)
        })
        .collect();
    if keyed.len() != indices.len() {
        return;
    }
    keyed.sort_by(|((na, sa), _), ((nb, sb), _)| {
        let ord = compare_sort_keys(*na, sa, *nb, sb);
        if desc {
            ord.reverse()
        } else {
            ord
        }
    });
    for (slot, (_, i)) in indices.iter_mut().zip(keyed) {
        *slot = i;
    }
}

// A mixed numeric/text comparator must keep the two kinds in a fixed order.
// Falling back to text only for mixed pairs creates cycles (2 < 10 < 11x < 2).
pub(crate) fn compare_sort_keys(a: Option<f64>, sa: &str, b: Option<f64>, sb: &str) -> std::cmp::Ordering {
    match (a.filter(|n| n.is_finite()), b.filter(|n| n.is_finite())) {
        (Some(a), Some(b)) => a.total_cmp(&b).then_with(|| sa.cmp(sb)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => sa.cmp(sb),
    }
}

pub fn query(
    events: &[Event],
    filters: &[Filter],
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
) -> QueryResult {
    let selected = select_memory_page(events, filters, sort_column, sort_dir, offset, limit);
    let rows = selected.ids.into_iter().map(|i| {
        let mut event = events[i].clone();
        crate::entities::annotate(&mut event);
        event.raw = String::new();
        event
    }).collect();
    QueryResult { total: selected.total.unwrap_or(0), rows }
}

fn select_memory_page(events: &[Event], filters: &[Filter], sort_column: &str, sort_dir: &str,
    offset: usize, limit: usize) -> SelectedPage {
    let mut ids = filtered_indices(events, filters);
    if !sort_column.is_empty() { sort_indices(events, &mut ids, sort_column, sort_dir == "desc"); }
    let total = ids.len();
    SelectedPage::exact(ids.into_iter().skip(offset).take(limit).collect(), total, offset, "memory", None)
}

pub(crate) fn query_projected_memory(events: &[Event], filters: &[Filter], sort_column: &str,
    sort_dir: &str, offset: usize, plan: &crate::page_projection::ProjectionPlan,
) -> Result<crate::page_projection::ProjectedPage, String> {
    let selected = select_memory_page(events, filters, sort_column, sort_dir, offset, plan.limit);
    let items = selected.ids.iter().map(|&i| {
        crate::operations::check()?;
        plan.project_memory(&events[i])
    }).collect::<Result<Vec<_>, String>>()?;
    plan.finish(selected, items)
}

fn query_from_memory_matches(
    events: &[Event],
    mut matched: Vec<usize>,
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
) -> QueryResult {
    if !sort_column.is_empty() {
        sort_indices(events, &mut matched, sort_column, sort_dir == "desc");
    }
    let total = matched.len();
    let rows = matched
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|i| {
            let mut event = events[i].clone();
            crate::entities::annotate(&mut event);
            event.raw = String::new();
            event
        })
        .collect();
    QueryResult { total, rows }
}

fn aggregate_from_memory_matches(
    events: &[Event],
    matched: &[usize],
    group_column: &str,
    specs: &[AggSpec],
) -> AggResult {
    let mut budget = AnalyticsBudget::new();
    let mut groups: HashMap<Option<String>, Vec<Acc>> = HashMap::new();
    let mut order = Vec::new();
    for &i in matched {
        if groups.len() > group_budget(specs) { return AggResult::budget_error(); }
        if crate::operations::cancelled() {
            break;
        }
        let event = &events[i];
        if let Err(error) = push_group(
            &mut groups,
            &mut order,
            event,
            event.col_str(group_column),
            specs, &mut budget) { return AggResult::failure(error); }
    }
    build_agg_result(groups, order, group_column, specs)
}

pub fn explore(
    events: &[Event],
    filters: &[Filter],
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
) -> ExplorerSnapshot {
    let matched = filtered_indices(events, filters);
    let stats = stats_from(matched.iter().map(|&i| {
        let event = &events[i];
        (event.timestamp, event.level.as_str())
    }));
    let count = [AggSpec {
        func: "count".into(),
        column: "*".into(),
        alias: "n".into(),
    }];
    let sources = aggregate_from_memory_matches(events, &matched, "source", &count);
    let codes = aggregate_from_memory_matches(events, &matched, "code", &count);
    let query = query_from_memory_matches(events, matched, sort_column, sort_dir, offset, limit);
    ExplorerSnapshot {
        query,
        stats,
        sources,
        codes,
    }
}

// ==========================================================================
// Caminho indexado (arquivos grandes): filtra por metadados e só
// materializa o necessário.
// ==========================================================================

enum Tri {
    Pass,
    Fail,
    NeedEvent,
}

pub(crate) fn ci_contains_bytes(hay: &[u8], needle_lower: &[u8]) -> bool {
    if !hay.is_ascii() || !needle_lower.is_ascii() {
        return String::from_utf8_lossy(hay)
            .to_lowercase()
            .contains(String::from_utf8_lossy(needle_lower).as_ref());
    }
    let Some((&first, rest)) = needle_lower.split_first() else {
        return true;
    };
    let mut from = 0usize;
    // memchr2 no primeiro byte (duas caixas) e confirma o restante — ordens de
    // grandeza mais rápido que varrer byte a byte.
    while let Some(p) = if first.is_ascii_alphabetic() {
        memchr::memchr2(first, first.to_ascii_uppercase(), &hay[from..])
    } else {
        memchr::memchr(first, &hay[from..])
    } {
        let i = from + p;
        let end = i + needle_lower.len();
        if end <= hay.len() && hay[i + 1..end].eq_ignore_ascii_case(rest) {
            return true;
        }
        from = i + 1;
        if from >= hay.len() {
            break;
        }
    }
    false
}

fn meta_check(pf: &PreparedFilter, meta: &LineMeta, line: &[u8], enriched: bool) -> Tri {
    let f = &pf.f;
    let op = f.op.as_str();
    if op == "query" {
        let Some(expr) = &pf.expr else { return Tri::Fail };
        // Catalog names/descriptions and derived fields are absent from the
        // raw line; free text that may match them needs the parsed event.
        if enriched {
            return Tri::NeedEvent;
        }
        let escaped = memchr::memchr(b'\\', line).is_some();
        return match expr.line_check(meta, line, escaped) {
            Some(true) => Tri::Pass,
            Some(false) => Tri::Fail,
            None => Tri::NeedEvent,
        };
    }
    // Exact selection uses decoded field values, including whitespace and case.
    // Raw spans can contain JSON escapes or represent a normalized standard field.
    if matches!(op, "equals_exact" | "not_equals_exact" | "threat_rule") {
        return Tri::NeedEvent;
    }
    let v = f.value.trim();
    // equivalente a v.to_lowercase() (trim e lowercase comutam para whitespace)
    let needle = pf.needle_lower.trim();
    if matches!(f.column.as_str(), "message" | "_all") && memchr::memchr(b'\\', line).is_some() {
        return Tri::NeedEvent;
    }
    match f.column.as_str() {
        "timestamp" => match op {
            "gt" | "gte" | "lt" | "lte" | "between" => {
                if meta.ts == 0 {
                    return Tri::Fail;
                }
                let a = meta.ts as f64;
                // limites pré-computados em prepare (pf.num/pf.num2)
                let pass = match op {
                    "gt" => pf.num.map(|b| a > b),
                    "gte" => pf.num.map(|b| a >= b),
                    "lt" => pf.num.map(|b| a < b),
                    "lte" => pf.num.map(|b| a <= b),
                    _ => match (pf.num, pf.num2) {
                        (Some(lo), Some(hi)) => Some(a >= lo && a <= hi),
                        _ => None,
                    },
                };
                match pass {
                    Some(true) => Tri::Pass,
                    Some(false) => Tri::Fail,
                    None => Tri::NeedEvent,
                }
            }
            "empty" => {
                if meta.ts == 0 {
                    Tri::Pass
                } else {
                    Tri::Fail
                }
            }
            "not_empty" => {
                if meta.ts != 0 {
                    Tri::Pass
                } else {
                    Tri::Fail
                }
            }
            _ => Tri::NeedEvent,
        },
        "level" => match (op, label_class(v)) {
            ("equals", Some(c)) => {
                if meta.level == c {
                    Tri::Pass
                } else if meta.level == LV_OTHER {
                    Tri::NeedEvent
                } else {
                    Tri::Fail
                }
            }
            ("not_equals", Some(c)) => {
                if meta.level == c {
                    Tri::Fail
                } else if meta.level == LV_OTHER {
                    Tri::NeedEvent
                } else {
                    Tri::Pass
                }
            }
            _ => Tri::NeedEvent,
        },
        "code" => {
            if meta.code_len == 0 {
                return Tri::NeedEvent;
            }
            let code = meta.code(line);
            match op {
                "equals" | "not_equals" => {
                    let eq = code.eq_ignore_ascii_case(v.as_bytes());
                    if (op == "equals") == eq {
                        Tri::Pass
                    } else {
                        Tri::Fail
                    }
                }
                "contains" | "not_contains" => {
                    if v.is_empty() {
                        return Tri::NeedEvent;
                    }
                    let has = !code.is_empty() && ci_contains_bytes(code, needle.as_bytes());
                    if (op == "contains") == has {
                        Tri::Pass
                    } else {
                        Tri::Fail
                    }
                }
                "empty" => {
                    if code.is_empty() {
                        Tri::Pass
                    } else {
                        Tri::Fail
                    }
                }
                "not_empty" => {
                    if !code.is_empty() {
                        Tri::Pass
                    } else {
                        Tri::Fail
                    }
                }
                _ => Tri::NeedEvent,
            }
        }
        "message" => match op {
            "contains" | "not_contains" => {
                if v.is_empty() {
                    return Tri::NeedEvent;
                }
                let has = ci_contains_bytes(line, needle.as_bytes());
                if !has {
                    // certeza sem parsear
                    if op == "contains" {
                        Tri::Fail
                    } else {
                        Tri::Pass
                    }
                } else {
                    Tri::NeedEvent // confirmar no evento parseado
                }
            }
            // Anchors refer to the parsed message, not the surrounding JSON.
            "regex" => Tri::NeedEvent,
            _ => Tri::NeedEvent,
        },
        "_all" => match op {
            "regex" => Tri::NeedEvent,
            "contains" | "not_contains" => {
                if v.is_empty() {
                    return Tri::NeedEvent;
                }
                let has = ci_contains_bytes(line, needle.as_bytes());
                if (op == "contains") == has {
                    Tri::Pass
                } else {
                    Tri::Fail
                }
            }
            _ => Tri::NeedEvent,
        },
        _ => Tri::NeedEvent,
    }
}

/// Complete-ID collection is a compatibility API with an explicit result
/// budget. The database selection cache owns shared work; no second Rust copy
/// is retained under the old pointer/count-based line-cache identity.
pub fn clear_match_cache() {}

pub(crate) fn check_collected_ids(rows: usize) -> Result<(), String> {
    if rows.saturating_mul(std::mem::size_of::<usize>()) > crate::resources::collected_ids_bytes() {
        Err("A lista completa de IDs excedeu LOGINSIGHT_COLLECTED_IDS_MB. Use paginação ou uma operação de leitura em fluxo.".into())
    } else { Ok(()) }
}
pub(crate) fn push_collected_id(ids: &mut Vec<usize>, id: usize) -> Result<(), String> {
    check_collected_ids(ids.len().saturating_add(1))?;
    if ids.len() == ids.capacity() {
        let left = crate::resources::collected_ids_bytes() / std::mem::size_of::<usize>() - ids.len();
        ids.try_reserve_exact(left.min(8192)).map_err(|e| e.to_string())?;
    }
    ids.push(id);
    Ok(())
}
/// Estimate owned event payload without serializing a second copy. This includes
/// retained string capacity and overlay provenance, not all allocator/RSS costs.
pub(crate) fn event_payload_bytes(event: &Event) -> usize {
    fn value_bytes(value: &Value) -> usize {
        match value {
            Value::String(value) => value.capacity().saturating_add(32),
            Value::Array(values) => values.iter().fold(32usize, |n, v| n.saturating_add(value_bytes(v))),
            Value::Object(values) => values.iter().fold(32usize, |n, (k, v)| n.saturating_add(k.capacity()).saturating_add(64).saturating_add(value_bytes(v))),
            _ => 32,
        }
    }
    [&event.event_ref, &event.parse_status, &event.source, &event.level, &event.code, &event.name, &event.description, &event.message, &event.raw]
        .iter().fold(std::mem::size_of::<Event>(), |n, v| n.saturating_add(v.capacity()))
        .saturating_add(event.fields.iter().fold(0usize, |n, (k, v)| n.saturating_add(k.capacity()).saturating_add(64).saturating_add(value_bytes(v))))
        .saturating_add(event.derived_originals.iter().fold(0usize, |n, (key, original)| {
            n.saturating_add(key.capacity()).saturating_add(64).saturating_add(match original {
                crate::model::DerivedOriginal::Missing => 32,
                crate::model::DerivedOriginal::Present(value) => value_bytes(value),
            })
        }))
        .saturating_add(event.derived_diagnostics.capacity().saturating_mul(std::mem::size_of::<crate::model::DerivedDiagnostic>()))
        .saturating_add(event.derived_diagnostics.iter().fold(0usize, |n, diagnostic| {
            n.saturating_add(diagnostic.field.capacity()).saturating_add(diagnostic.code.capacity()).saturating_add(diagnostic.message.capacity())
        }))
        .saturating_add(event.evidence_provenance.as_ref().map_or(0, |proof| {
            proof.source.version.capacity().saturating_add(proof.source.record_space.capacity()).saturating_add(proof.source.label.capacity())
                .saturating_add(proof.source.event_ref_prefix.as_ref().map_or(0, String::capacity))
                .saturating_add(match &proof.locator { crate::exclusion_store::Locator::StableRecord(key) => key.capacity(), _ => 0 }).saturating_add(128)
        }))
}

#[cfg(test)]
mod event_payload_budget_tests {
    use super::*;

    #[test]
    fn hidden_derived_originals_and_diagnostics_share_the_event_budget() {
        let mut event = Event::empty();
        event.fields.insert("payload".into(), Value::from("small replacement"));
        let baseline = event_payload_bytes(&event);
        event.derived_originals.insert("payload".into(), crate::model::DerivedOriginal::Present(
            serde_json::json!({"nested":["日".repeat(24_000), {"original":"x".repeat(4_096)}]})
        ));
        event.derived_originals.insert("new_child".into(), crate::model::DerivedOriginal::Missing);
        event.derived_diagnostics.push(crate::model::DerivedDiagnostic {
            field:"payload".into(), code:"transform_input_limit".into(), message:"d".repeat(8_192), warning:false,
        });
        let before = serde_json::to_value(&event).unwrap();
        let measured = event_payload_bytes(&event);
        assert!(measured >= baseline + 72_000 + 4_096 + 8_192);
        let mut budget = AnalyticsBudget { used:0, limit:32 << 10 };
        assert!(budget.charge(measured).is_err());
        assert_eq!(serde_json::to_value(&event).unwrap(), before, "accounting cannot strip evidence or diagnostics");
    }

    #[test]
    fn retained_empty_string_capacity_remains_accounted_until_released() {
        let mut event = Event::empty();
        let baseline = event_payload_bytes(&event);
        event.raw = "x".repeat(64 << 10);
        event.raw.clear();
        assert!(event_payload_bytes(&event) >= baseline + (64 << 10));
        event.raw = String::new();
        assert_eq!(event_payload_bytes(&event), baseline);
    }

    #[test]
    fn memory_page_routes_release_raw_without_mutating_saved_evidence() {
        let mut original = Event::empty();
        original.raw = "x".repeat(64 << 10);
        original.fields.insert("body".into(), Value::from("日".repeat(1_024)));
        original.event_ref = "saved:1".into();
        let events = vec![original];
        let before = serde_json::to_value(&events).unwrap();
        for page in [query(&events, &[], "id", "asc", 0, 1), query_from_memory_matches(&events, vec![0], "id", "asc", 0, 1)] {
            assert_eq!(page.rows.len(), 1);
            assert!(page.rows[0].raw.is_empty());
            assert_eq!(page.rows[0].raw.capacity(), 0);
            assert_eq!(page.rows[0].fields["body"], events[0].fields["body"]);
            assert_eq!(page.rows[0].event_ref, events[0].event_ref);
        }
        assert_eq!(serde_json::to_value(&events).unwrap(), before);
        assert_eq!(events[0].raw.len(), 64 << 10);
    }
}
pub fn indexed_matches(idx: &FileIndex, filters: &[Filter], codes: &CodesConfig, system: &CodesConfig, derived: &[CompiledDerived]) -> Result<Vec<usize>, String> {
    if filters.is_empty() {
        let gate = crate::analysis_runtime::indexed_gate(idx)?;
        check_collected_ids(gate.as_ref().map_or(idx.lines.len(), |gate| gate.visible_count()))?;
        return Ok((0..idx.lines.len()).filter(|&id| gate.as_ref().is_none_or(|gate| gate.allows_known_row(id))).collect());
    }
    let pfs = prepare(filters);
    if let Some(ids) = crate::engine::matches(&engine_source(idx, codes, system, derived), &pfs)? { return Ok(ids); }
    let mut ids = Vec::new();
    let mut error = None;
    scan_indexed_control(idx, &pfs, codes, system, derived, |i, _, _| i, |i| {
        if let Err(e) = push_collected_id(&mut ids, i) { error = Some(e); return false; }
        true
    })?;
    if let Some(error) = error { return Err(error); }
    crate::operations::check()?;
    Ok(ids)
}

fn engine_source<'a>(
    idx: &'a FileIndex,
    codes: &'a CodesConfig,
    system: &'a CodesConfig,
    derived: &'a [CompiledDerived],
) -> crate::engine::Source<'a> {
    crate::engine::Source { idx, codes, system, derived }
}

/// Visit matching positions without allocating a vector proportional to the file.
pub fn visit_indexed_matches(idx: &FileIndex, filters: &[Filter], codes: &CodesConfig, system: &CodesConfig, derived: &[CompiledDerived], visit: impl FnMut(usize)) -> Result<(), String> {
    visit_indexed_prepared(idx, &prepare(filters), codes, system, derived, visit)
}

pub(crate) fn visit_indexed_events(idx: &FileIndex, pfs: &[PreparedFilter], codes: &CodesConfig, system: &CodesConfig, derived: &[CompiledDerived], mut visit: impl FnMut(usize, &Event)) -> Result<(), String> {
    visit_indexed_mapped(idx, pfs, codes, system, derived, |_| (), |i, event, ()| visit(i, event))
}

/// Preserves file order while hydrating only a byte-bounded wave of IDs.
/// There is no preliminary vector proportional to the complete match set.
pub(crate) fn visit_indexed_mapped<T: Send>(idx: &FileIndex, pfs: &[PreparedFilter], codes: &CodesConfig, system: &CodesConfig, derived: &[CompiledDerived], map: impl Fn(&Event) -> T + Sync, mut visit: impl FnMut(usize, &Event, T)) -> Result<(), String> {
    let mut ids = Vec::new();
    let mut bytes = 0usize;
    let flush = |ids: &mut Vec<usize>, visit: &mut dyn FnMut(usize, &Event, T)| {
        let events: Vec<_> = ids.par_iter().map(|&id| {
            let event = event_at(idx, id, codes, system, derived);
            let mapped = map(&event);
            (id, event, mapped)
        }).collect();
        for (id, event, mapped) in events { visit(id, &event, mapped); }
        ids.clear();
    };
    visit_indexed_prepared_control(idx, pfs, codes, system, derived, |id| {
        let size = idx.lines.at(id).len as usize;
        if !ids.is_empty() && (ids.len() >= 8192 || bytes.saturating_add(size) > crate::resources::batch_bytes()) {
            flush(&mut ids, &mut visit);
            bytes = 0;
            crate::operations::check()?;
        }
        ids.push(id);
        bytes = bytes.saturating_add(size);
        Ok(true)
    })?;
    crate::operations::check()?;
    flush(&mut ids, &mut visit);
    crate::operations::check()
}

/// Lines per parallel work unit. Batches keep memory bounded and results in order.
const SCAN_CHUNK: usize = 8192;

/// Scans the index in parallel chunks, calling `map` for each matching line
/// (with the parsed event when one was needed) and `visit` in file order.
pub(crate) fn scan_indexed<T: Send>(
    idx: &FileIndex,
    pfs: &[PreparedFilter],
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
    map: impl Fn(usize, &LineMeta, Option<Event>) -> T + Sync,
    mut visit: impl FnMut(T),
) -> Result<(), String> {
    scan_indexed_control(idx, pfs, codes, system, derived, map, |item| { visit(item); true })
}

fn scan_indexed_control<T: Send>(
    idx: &FileIndex,
    pfs: &[PreparedFilter],
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
    map: impl Fn(usize, &LineMeta, Option<Event>) -> T + Sync,
    mut visit: impl FnMut(T) -> bool,
) -> Result<(), String> {
    let gate = crate::analysis_runtime::indexed_gate(idx)?;
    let enriched: Vec<bool> = pfs
        .iter()
        .map(|pf| query_needs_enrichment(pf, codes, system, derived))
        .collect();
    let cancellation = crate::operations::current_token();
    let chunk = |from: usize, to: usize| -> Vec<T> {
        let mut out = Vec::new();
        for i in from..to {
            if (i - from) % 2048 == 0 && cancellation.cancelled() {
                break;
            }
            if gate.as_ref().is_some_and(|gate| !gate.allows_known_row(i)) { continue; }
            let meta = &idx.lines.at(i);
            let line = line_bytes(idx, i);
            let mut need = false;
            let mut ok = true;
            for (pf, enriched) in pfs.iter().zip(&enriched) {
                match meta_check(pf, meta, line, *enriched) {
                    Tri::Fail => {
                        ok = false;
                        break;
                    }
                    Tri::NeedEvent => need = true,
                    Tri::Pass => {}
                }
            }
            if !ok {
                continue;
            }
            if need {
                let ev = event_at(idx, i, codes, system, derived);
                if pfs.iter().all(|pf| matches_indexed(&ev, pf)) {
                    out.push(map(i, meta, Some(ev)));
                }
            } else {
                out.push(map(i, meta, None));
            }
        }
        out
    };
    let total = idx.lines.len();
    let batch = SCAN_CHUNK * rayon::current_num_threads() * 2;
    let mut start = 0;
    while start < total {
        if crate::operations::cancelled() {
            break;
        }
        let mut end = start;
        let mut bytes = 0usize;
        while end < total && end - start < batch {
            let next = idx.lines.at(end).len as usize;
            if end > start && bytes.saturating_add(next) > crate::resources::batch_bytes() { break; }
            bytes = bytes.saturating_add(next);
            end += 1;
        }
        let parts: Vec<Vec<T>> = (start..end)
            .step_by(SCAN_CHUNK)
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|from| chunk(from, (from + SCAN_CHUNK).min(end)))
            .collect();
        for part in parts {
            for item in part {
                if !visit(item) { return crate::operations::check(); }
            }
        }
        start = end;
    }
    crate::operations::check()
}

pub(crate) fn visit_indexed_prepared(idx: &FileIndex, pfs: &[PreparedFilter], codes: &CodesConfig, system: &CodesConfig, derived: &[CompiledDerived], mut visit: impl FnMut(usize)) -> Result<(), String> {
    visit_indexed_prepared_control(idx, pfs, codes, system, derived, |id| { visit(id); Ok(true) })
}

pub(crate) fn visit_indexed_prepared_control(idx: &FileIndex, pfs: &[PreparedFilter], codes: &CodesConfig, system: &CodesConfig, derived: &[CompiledDerived], mut visit: impl FnMut(usize) -> Result<bool, String>) -> Result<(), String> {
    let gate = crate::analysis_runtime::indexed_gate(idx)?;
    if pfs.is_empty() {
        for id in 0..idx.lines.len() {
            if id % 2048 == 0 { crate::operations::check()?; }
            if gate.as_ref().is_some_and(|gate| !gate.allows_known_row(id)) { continue; }
            if !visit(id)? { break; }
        }
        return crate::operations::check();
    }
    if crate::engine::visit_matches(&engine_source(idx, codes, system, derived), pfs, &mut visit)?.is_some() { return Ok(()); }
    let mut failure = None;
    scan_indexed_control(idx, pfs, codes, system, derived, |i, _, _| i, |id| match visit(id) {
        Ok(keep) => keep,
        Err(error) => { failure = Some(error); false }
    })?;
    if let Some(error) = failure { return Err(error); }
    crate::operations::check()
}


/// the catalogs or derived fields, which are not part of the raw line.
fn query_needs_enrichment(
    pf: &PreparedFilter,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> bool {
    let Some(expr) = &pf.expr else { return false };
    let mut needles = Vec::new();
    expr.free_text_needles(&mut needles);
    if needles.is_empty() {
        return false;
    }
    if !derived.is_empty() {
        return true;
    }
    [codes, system].iter().any(|catalog| {
        catalog.sources.values().flat_map(|m| m.values()).any(|info| {
            let name = info.name.to_lowercase();
            let description = info.description.to_lowercase();
            needles.iter().any(|n| name.contains(n.as_str()) || description.contains(n.as_str()))
        })
    })
}

/// Reuses the line engine's per-filter raw/meta decision without scanning
/// unrelated rows. SQL already checked exact predicates; only residual tests
/// belong here. A candidate is hydrated at most once, even with several tests.
pub(crate) struct CandidateVerifier<'a> {
    visibility: Option<std::sync::Arc<crate::analysis_runtime::RowGate>>,
    idx: &'a FileIndex,
    pfs: &'a [PreparedFilter],
    lines: Vec<(usize, bool)>,
    verify: &'a [usize],
    codes: &'a CodesConfig,
    system: &'a CodesConfig,
    derived: &'a [CompiledDerived],
}
impl<'a> CandidateVerifier<'a> {
    pub(crate) fn new(idx: &'a FileIndex, pfs: &'a [PreparedFilter], lines: &[usize], verify: &'a [usize], codes: &'a CodesConfig, system: &'a CodesConfig, derived: &'a [CompiledDerived]) -> Result<Self, String> {
        Ok(Self { visibility: crate::analysis_runtime::indexed_gate(idx)?, idx, pfs, lines: lines.iter().map(|&i| (i, query_needs_enrichment(&pfs[i], codes, system, derived))).collect(), verify, codes, system, derived })
    }
    pub(crate) fn matches(&self, id: usize) -> bool {
        if self.visibility.as_ref().is_some_and(|gate| !gate.allows_known_row(id)) { return false; }
        let meta = &self.idx.lines.at(id);
        let raw = line_bytes(self.idx, id);
        let mut event = None;
        for &(i, enriched) in &self.lines {
            match meta_check(&self.pfs[i], meta, raw, enriched) {
                Tri::Fail => return false,
                Tri::Pass => (),
                Tri::NeedEvent => {
                    let ev = event.get_or_insert_with(|| event_at(self.idx, id, self.codes, self.system, self.derived));
                    if !matches_indexed(ev, &self.pfs[i]) { return false; }
                }
            }
        }
        if !self.verify.is_empty() {
            let ev = event.get_or_insert_with(|| event_at(self.idx, id, self.codes, self.system, self.derived));
            if !self.verify.iter().all(|&i| matches_indexed(ev, &self.pfs[i])) { return false; }
        }
        true
    }
}

pub fn query_indexed(
    idx: &FileIndex,
    filters: &[Filter],
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> Result<QueryResult, String> {
    if let Some(result) = crate::engine::query(
        &engine_source(idx, codes, system, derived),
        &prepare(filters),
        sort_column,
        sort_dir,
        offset,
        limit,
    )? {
        return Ok(result);
    }
    let page = query_page_lines(idx, filters, sort_column, sort_dir, offset, limit, codes, system, derived)?;
    let total = count_lines(idx, filters, codes, system, derived)?;
    crate::operations::check()?;
    Ok(QueryResult { total, rows: page.rows })
}

#[allow(clippy::too_many_arguments)]
pub fn query_page_indexed(
    idx: &FileIndex,
    filters: &[Filter],
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
    cursor: Option<&str>,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> Result<QueryPage, String> {
    let _interactive = crate::operations::interactive();
    if let Some(result) = crate::engine::query_page(
        &engine_source(idx, codes, system, derived), &prepare(filters),
        sort_column, sort_dir, offset, limit, cursor,
    ) {
        return result;
    }
    if cursor.is_some() {
        return Err("PAGINATION_RESET_REQUIRED: O índice desta paginação não está disponível. Recarregue a primeira página para usar a recuperação.".into());
    }
    // Fallback remains explicit in the response; consumers can distinguish a
    // ready indexed page from slower recovery while stores are being prepared.
    query_page_lines(idx, filters, sort_column, sort_dir, offset, limit, codes, system, derived)
}

#[derive(Debug)]
enum RecoveryKey {
    Integer(i64),
    Text(Option<f64>, String),
}

#[derive(Debug)]
struct RecoveryRow { id: usize, key: RecoveryKey, desc: bool }

impl RecoveryRow {
    fn bytes(&self) -> usize {
        std::mem::size_of::<Self>() + match &self.key { RecoveryKey::Text(_, text) => text.len(), _ => 0 }
    }
}

impl PartialEq for RecoveryRow { fn eq(&self, other: &Self) -> bool { self.cmp(other).is_eq() } }
impl Eq for RecoveryRow {}
impl PartialOrd for RecoveryRow { fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> { Some(self.cmp(other)) } }
impl Ord for RecoveryRow {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let order = match (&self.key, &other.key) {
            (RecoveryKey::Integer(a), RecoveryKey::Integer(b)) => a.cmp(b),
            (RecoveryKey::Text(a, at), RecoveryKey::Text(b, bt)) => compare_sort_keys(*a, at, *b, bt),
            _ => self.id.cmp(&other.id),
        };
        (if self.desc { order.reverse() } else { order }).then(self.id.cmp(&other.id))
    }
}

/// Recovery is slower, but never allocates one id or sort key per matching
/// event. Deep random access waits for the persistent index instead of risking
/// a desktop OOM. Ordinary forward indexed navigation uses cursors above.
#[allow(clippy::too_many_arguments)]
pub(crate) fn query_page_lines(
    idx: &FileIndex, filters: &[Filter], sort_column: &str, sort_dir: &str,
    offset: usize, limit: usize, codes: &CodesConfig, system: &CodesConfig,
    derived: &[CompiledDerived],
) -> Result<QueryPage, String> {
    let selected = select_page_lines(idx, filters, sort_column, sort_dir, offset, limit, codes, system, derived)?;
    let binding = crate::analysis_runtime::source_set(idx)?;
    let rows = selected.ids.iter().map(|&id| {
        let mut event = event_at(idx, id, codes, system, derived);
        crate::analysis_runtime::attach_provenance_with(idx, &binding, &mut event)?;
        crate::entities::annotate(&mut event);
        event.raw = String::new();
        Ok(event)
    }).collect::<Result<Vec<_>, String>>()?;
    Ok(selected.into_full(rows))
}

#[allow(clippy::too_many_arguments)]
fn select_page_lines(
    idx: &FileIndex, filters: &[Filter], sort_column: &str, sort_dir: &str,
    offset: usize, limit: usize, codes: &CodesConfig, system: &CodesConfig,
    derived: &[CompiledDerived],
) -> Result<SelectedPage, String> {
    for part in &idx.parts { crate::sources::validate_source(part)?; }
    const WINDOW: usize = 10_000;
    const KEY_BUDGET: usize = 32 << 20;
    let keep = offset.saturating_add(limit).saturating_add(1);
    if keep > WINDOW {
        return Err("Navegação profunda aguarda o índice de consultas. Reduza o recorte ou conclua/repare a preparação.".into());
    }
    let gate = crate::analysis_runtime::indexed_gate(idx)?;
    let pfs = prepare(filters);
    let enriched: Vec<_> = pfs.iter().map(|pf| query_needs_enrichment(pf, codes, system, derived)).collect();
    let mut selected: std::collections::BinaryHeap<RecoveryRow> = std::collections::BinaryHeap::new();
    let (mut total, mut bytes) = (0usize, 0usize);
    for i in 0..idx.lines.len() {
        if i % 2048 == 0 { crate::operations::check()?; }
        if gate.as_ref().is_some_and(|gate| !gate.allows_known_row(i)) { continue; }
        let meta = &idx.lines.at(i);
        let line = line_bytes(idx, i);
        let mut needs_event = false;
        let mut eligible = true;
        for (pf, enriched) in pfs.iter().zip(&enriched) {
            match meta_check(pf, meta, line, *enriched) {
                Tri::Fail => { eligible = false; break; }
                Tri::NeedEvent => needs_event = true,
                Tri::Pass => {}
            }
        }
        if !eligible { continue; }
        let mut event = needs_event.then(|| event_at(idx, i, codes, system, derived));
        if event.as_ref().is_some_and(|event| !pfs.iter().all(|pf| matches_indexed(event, pf))) { continue; }
        total += 1;
        let key = match sort_column {
            "" | "id" => RecoveryKey::Integer(i as i64),
            "timestamp" => RecoveryKey::Integer(meta.ts),
            "level" => RecoveryKey::Integer(meta.level as i64),
            column => {
                let event = event.get_or_insert_with(|| event_at(idx, i, codes, system, derived));
                let text = event.col_ref(column).unwrap_or_default().to_lowercase();
                if text.len() > KEY_BUDGET { return Err("Campo de ordenação excede o orçamento de recuperação. Escolha outra coluna ou conclua a preparação.".into()); }
                RecoveryKey::Text(event.col_num(column), text)
            }
        };
        let item = RecoveryRow { id: i, key, desc: !sort_column.is_empty() && sort_dir == "desc" };
        if selected.len() < keep || selected.peek().is_some_and(|worst| item < *worst) {
            if selected.len() == keep { bytes = bytes.saturating_sub(selected.pop().unwrap().bytes()); }
            bytes = bytes.saturating_add(item.bytes());
            if bytes > KEY_BUDGET { return Err("Ordenação excede o orçamento de recuperação. Reduza o recorte ou conclua a preparação.".into()); }
            selected.push(item);
        }
    }
    crate::operations::check()?;
    let ids = selected.into_sorted_vec().into_iter().skip(offset).take(limit).map(|row| row.id).collect();
    Ok(SelectedPage::exact(ids, total, offset, "lines", Some(
        "Modo de recuperação: índice indisponível, leitura mais lenta e navegação limitada a 10 mil registros.".into(),
    )))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn query_projected_indexed(
    idx: &FileIndex, filters: &[Filter], sort_column: &str, sort_dir: &str,
    offset: usize, cursor: Option<&str>, codes: &CodesConfig, system: &CodesConfig,
    derived: &[CompiledDerived], plan: &crate::page_projection::ProjectionPlan,
) -> Result<crate::page_projection::ProjectedPage, String> {
    let _interactive = crate::operations::interactive();
    if let Some(result) = crate::engine::query_projected_page(
        &engine_source(idx, codes, system, derived), &prepare(filters),
        sort_column, sort_dir, offset, cursor, plan,
    ) { return result; }
    if cursor.is_some() {
        return Err("PAGINATION_RESET_REQUIRED: O índice desta paginação não está disponível. Recarregue a primeira página para usar a recuperação.".into());
    }
    let selected = select_page_lines(idx, filters, sort_column, sort_dir, offset, plan.limit, codes, system, derived)?;
    let items = selected.ids.iter().map(|&id| crate::page_projection::project_indexed_row(idx, id, codes, system, derived, plan))
        .collect::<Result<Vec<_>, String>>()?;
    plan.finish(selected, items)
}

fn sort_time(idx: &FileIndex, matched: &mut Vec<usize>, desc: bool) {
    if matched.len() > idx.lines.len() / 4 {
        if matched.len() == idx.lines.len() {
            *matched = idx.ordered().to_vec();
        } else {
            let mut selected = vec![false; idx.lines.len()];
            for &i in matched.iter() {
                selected[i] = true;
            }
            *matched = idx
                .ordered()
                .iter()
                .copied()
                .filter(|&i| selected[i])
                .collect();
        }
        if desc {
            matched.reverse();
            let mut start = 0;
            while start < matched.len() {
                let mut end = start + 1;
                while end < matched.len()
                    && idx.lines.at(matched[end]).ts == idx.lines.at(matched[start]).ts
                {
                    end += 1;
                }
                matched[start..end].reverse();
                start = end;
            }
        }
    } else {
        matched.sort_by(|&a, &b| {
            let ord = idx.lines.at(a).ts.cmp(&idx.lines.at(b).ts);
            if desc {
                ord.reverse()
            } else {
                ord
            }
        });
    }
}

// ==========================================================================
// Agregações
// ==========================================================================

pub(crate) struct AnalyticsBudget { used: usize, limit: usize }
impl AnalyticsBudget {
    pub(crate) fn new() -> Self { Self { used: 0, limit: crate::resources::analytics_bytes() } }
    pub(crate) fn charge(&mut self, bytes: usize) -> Result<(), String> {
        self.used = self.used.saturating_add(bytes);
        if self.used > self.limit { Err("O resultado analítico excedeu o orçamento de valores (LOGINSIGHT_ANALYTICS_LIMIT_MB). Restrinja os filtros ou reduza os agrupamentos.".into()) } else { Ok(()) }
    }
    pub(crate) fn group(&mut self, key: &Option<String>, specs: usize) -> Result<(), String> {
        self.charge(key.as_ref().map_or(0, |v| v.len()).saturating_mul(3).saturating_add(128).saturating_add(specs.saturating_mul(std::mem::size_of::<Acc>())))
    }
}

#[cfg(test)]
mod analytics_budget_tests {
    use super::*;
    #[test]
    fn group_labels_and_text_values_share_one_byte_budget() {
        let mut budget = AnalyticsBudget { used: 0, limit: 1024 };
        budget.group(&Some("日".repeat(40)), 1).unwrap();
        assert!(budget.group(&Some("x".repeat(400)), 1).is_err());
        let mut budget = AnalyticsBudget { used: 0, limit: 64 };
        let mut acc = Acc::StrAgg(Vec::new());
        let mut event = Event::empty();
        event.message = "日".repeat(30);
        assert!(acc.push(&event, "message", &mut budget).is_err());
        assert!(matches!(acc, Acc::StrAgg(ref values) if values.is_empty()));
    }
    #[test]
    fn distinct_groups_charge_only_new_values_to_the_shared_budget() {
        let mut budget = AnalyticsBudget { used: 0, limit: 200 };
        let mut a = Acc::CountDistinct(Default::default());
        let mut b = Acc::CountDistinct(Default::default());
        let mut event = Event::empty();
        event.message = "same".into();
        a.push(&event, "message", &mut budget).unwrap();
        let used = budget.used;
        a.push(&event, "message", &mut budget).unwrap();
        assert_eq!(used, budget.used);
        b.push(&event, "message", &mut budget).unwrap();
        event.message = "another".into();
        assert!(b.push(&event, "message", &mut budget).is_err());
    }
}

pub(crate) enum Acc {
    Count(u64),
    CountDistinct(crate::distinct::Counter),
    /// Distinct count computed elsewhere (query engine).
    Distinct(u64),
    Sum(f64, [usize; 5]),
    Avg(f64, u64, [usize; 5]),
    Min(Option<f64>, [usize; 5]),
    Max(Option<f64>, [usize; 5]),
    StrAgg(Vec<String>),
}

impl Acc {
    fn new(func: &str) -> Self {
        match func {
            "count" => Acc::Count(0),
            "count_distinct" => Acc::CountDistinct(Default::default()),
            "sum" => Acc::Sum(0.0, [0; 5]),
            "avg" => Acc::Avg(0.0, 0, [0; 5]),
            "min" => Acc::Min(None, [0; 5]),
            "max" => Acc::Max(None, [0; 5]),
            _ => Acc::StrAgg(Vec::new()),
        }
    }

    fn push(&mut self, ev: &Event, column: &str, budget: &mut AnalyticsBudget) -> Result<(), String> {
        let numeric = || {
            if matches!(column, "timestamp" | "id") {
                ev.col_num(column)
                    .map(|value| (value, crate::analysis::UnitKind::Number))
            } else {
                ev.col_ref(column)
                    .and_then(|value| crate::analysis::parse_num_unit(&value))
            }
        };
        match self {
            Acc::Count(n) => *n += 1,
            Acc::CountDistinct(set) => {
                if let Some(s) = ev.col_str(column) {
                    budget.charge(set.try_insert(s)?)?;
                }
            }
            Acc::Distinct(_) => {}
            Acc::Sum(n, units) => {
                if let Some((v, unit)) = numeric() {
                    *n += v;
                    units[unit as usize] += 1;
                }
            }
            Acc::Avg(sum, n, units) => {
                if let Some((v, unit)) = numeric() {
                    *sum += v;
                    *n += 1;
                    units[unit as usize] += 1;
                }
            }
            Acc::Min(cur, units) => {
                if let Some((v, unit)) = numeric() {
                    *cur = Some(cur.map(|c: f64| c.min(v)).unwrap_or(v));
                    units[unit as usize] += 1;
                }
            }
            Acc::Max(cur, units) => {
                if let Some((v, unit)) = numeric() {
                    *cur = Some(cur.map(|c: f64| c.max(v)).unwrap_or(v));
                    units[unit as usize] += 1;
                }
            }
            Acc::StrAgg(items) => {
                if items.len() < 100 {
                    if let Some(s) = ev.col_str(column) {
                        if !s.is_empty() {
                            budget.charge(s.len().saturating_mul(2).saturating_add(32))?;
                            items.push(s);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn units(&self) -> &[usize; 5] {
        match self {
            Acc::Sum(_, units)
            | Acc::Avg(_, _, units)
            | Acc::Min(_, units)
            | Acc::Max(_, units) => units,
            _ => &[0; 5],
        }
    }

    fn finish(&self, ev_col: &str) -> Value {
        match self {
            Acc::Count(n) => Value::from(*n),
            Acc::CountDistinct(set) => Value::from(set.len() as u64),
            Acc::Distinct(n) => Value::from(*n),
            Acc::Sum(n, _) => round2(*n).into(),
            Acc::Avg(sum, n, _) => {
                if *n == 0 {
                    Value::Null
                } else {
                    round2(*sum / *n as f64).into()
                }
            }
            Acc::Min(cur, _) | Acc::Max(cur, _) => match cur {
                Some(v) if ev_col == "timestamp" => Value::from(crate::model::ts_to_iso(*v as i64)),
                Some(v) => round2(*v).into(),
                None => Value::Null,
            },
            Acc::StrAgg(items) => {
                let mut s = items.join(", ");
                if items.len() >= 100 {
                    s.push_str(", …");
                }
                Value::from(s)
            }
        }
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

// ------------------------------------------------------------------ multi-agregação

/// Contagens por valor para várias colunas de uma vez (árvore de exploração).
/// Cada coluna é agregada com os filtros MENOS o filtro da própria coluna,
/// garantindo que toda opção exibida exista no recorte complementar.
/// As colunas são processadas em paralelo (rayon).
pub fn multi_count(
    events: &[Event],
    filters: &[Filter],
    columns: &[String],
) -> Vec<(String, AggResult)> {
    let cancellation = crate::operations::current_token();
    columns
        .par_iter()
        .map(|col| {
            let fs: Vec<Filter> = filters
                .iter()
                .filter(|f| f.column != *col)
                .cloned()
                .collect();
            let run = || {
                aggregate(
                    events,
                    &fs,
                    col,
                    &[AggSpec {
                        func: "count".into(),
                        column: "*".into(),
                        alias: "n".into(),
                    }],
                )
            };
            let agg = crate::operations::run_with_token(cancellation.clone(), run).unwrap_or_default();
            (col.clone(), agg)
        })
        .collect()
}

/// Versão indexada (mmap) da multi-agregação.
pub fn multi_count_indexed(
    idx: &FileIndex,
    filters: &[Filter],
    columns: &[String],
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> Vec<(String, AggResult)> {
    match crate::engine::multi_count(&engine_source(idx, codes, system, derived), filters, columns) {
        Ok(Some(result)) => return result,
        Err(error) => return columns.iter().map(|column| (column.clone(), AggResult::failure(error.clone()))).collect(),
        Ok(None) => (),
    }
    let cancellation = crate::operations::current_token();
    columns
        .par_iter()
        .map(|col| {
            let fs: Vec<Filter> = filters
                .iter()
                .filter(|f| f.column != *col)
                .cloned()
                .collect();
            let run = || {
                aggregate_indexed(
                    idx,
                    &fs,
                    col,
                    &[AggSpec {
                        func: "count".into(),
                        column: "*".into(),
                        alias: "n".into(),
                    }],
                    codes,
                    system,
                    derived,
                )
            };
            let agg = crate::operations::run_with_token(cancellation.clone(), run).unwrap_or_default();
            (col.clone(), agg)
        })
        .collect()
}

pub(crate) fn build_agg_result(
    groups: HashMap<Option<String>, Vec<Acc>>,
    mut order: Vec<Option<String>>,
    group_column: &str,
    specs: &[AggSpec],
) -> AggResult {
    if groups.len() > group_budget(specs) { return AggResult::budget_error(); }
    order.sort_by(|a, b| {
        let (sa, sb) = (a.as_deref().unwrap_or(""), b.as_deref().unwrap_or(""));
        compare_sort_keys(
            sa.parse().ok(),
            &sa.to_lowercase(),
            sb.parse().ok(),
            &sb.to_lowercase(),
        )
        .then_with(|| a.cmp(b))
    });

    let alias = |s: &AggSpec| {
        if s.alias.is_empty() {
            format!("{}({})", s.func, s.column)
        } else {
            s.alias.clone()
        }
    };
    let mut columns = vec![group_column.to_string()];
    columns.extend(specs.iter().map(&alias));
    let mut incompatible_units = vec![0; specs.len()];
    let mut value_units = vec![String::new(); specs.len()];
    for i in 0..specs.len() {
        let mut counts = [0usize; 5];
        for accs in groups.values() {
            for (total, count) in counts.iter_mut().zip(accs[i].units()) {
                *total += count;
            }
        }
        if let Some((unit, count)) = counts
            .iter()
            .enumerate()
            .max_by_key(|(unit, count)| (**count, std::cmp::Reverse(*unit)))
            .filter(|(_, count)| **count > 0)
        {
            incompatible_units[i] = counts.iter().sum::<usize>() - count;
            value_units[i] = ["number", "bytes", "bits", "duration", "number"][unit].into();
        }
    }

    // Only the largest groups by the first measure are returned.
    let mut omitted_groups = 0;
    let mut omitted_records = 0u64;
    if order.len() > MAX_GROUPS && !specs.is_empty() {
        let score = |key: &Option<String>| match groups[key][0].finish(&specs[0].column) {
            Value::Number(n) => n.as_f64().unwrap_or(f64::NEG_INFINITY),
            _ => f64::NEG_INFINITY,
        };
        let scores: Vec<f64> = order.iter().map(score).collect();
        let mut ranked: Vec<usize> = (0..order.len()).collect();
        ranked.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]).then(a.cmp(&b)));
        let mut kept = vec![false; order.len()];
        for &i in &ranked[..MAX_GROUPS] {
            kept[i] = true;
        }
        let counted = specs.iter().position(|s| s.func == "count");
        omitted_groups = order.len() - MAX_GROUPS;
        for (key, _) in order.iter().zip(&kept).filter(|(_, kept)| !**kept) {
            if let Some(Acc::Count(n)) = counted.map(|c| &groups[key][c]) {
                omitted_records += n;
            }
        }
        order = order.into_iter().zip(kept).filter(|(_, kept)| *kept).map(|(key, _)| key).collect();
    }

    let group_values = order.clone();
    let rows = order
        .into_iter()
        .map(|key| {
            let accs = &groups[&key];
            let mut row = Map::new();
            row.insert(
                group_column.to_string(),
                Value::from(key.unwrap_or_else(|| "(vazio)".into())),
            );
            for (i, (acc, spec)) in accs.iter().zip(specs.iter()).enumerate() {
                row.insert(
                    alias(spec),
                    if incompatible_units[i] > 0 {
                        Value::Null
                    } else {
                        acc.finish(&spec.column)
                    },
                );
            }
            row
        })
        .collect();

    AggResult {
        columns,
        rows,
        group_values,
        incompatible_units,
        value_units,
        omitted_groups,
        omitted_records,
        error: None,
    }
}

pub fn aggregate(
    events: &[Event],
    filters: &[Filter],
    group_column: &str,
    specs: &[AggSpec],
) -> AggResult {
    let idx = filtered_indices(events, filters);
    let mut budget = AnalyticsBudget::new();
    let mut groups: HashMap<Option<String>, Vec<Acc>> = HashMap::new();
    let mut order: Vec<Option<String>> = Vec::new();

    for i in idx {
        if groups.len() > group_budget(specs) { return AggResult::budget_error(); }
        if crate::operations::cancelled() {
            break;
        }
        let ev = &events[i];
        if let Err(error) = push_group(&mut groups, &mut order, ev, ev.col_str(group_column), specs, &mut budget) { return AggResult::failure(error); }
    }
    build_agg_result(groups, order, group_column, specs)
}

fn push_group(
    groups: &mut HashMap<Option<String>, Vec<Acc>>,
    order: &mut Vec<Option<String>>,
    ev: &Event,
    key: Option<String>,
    specs: &[AggSpec],
    budget: &mut AnalyticsBudget,
) -> Result<(), String> {
    let key = key.filter(|value| !value.trim().is_empty());
    // Caminho quente é o grupo já existente: get_mut evita clonar a chave
    // a cada linha; só clona ao criar um grupo novo.
    if let Some(accs) = groups.get_mut(&key) {
        for (acc, spec) in accs.iter_mut().zip(specs.iter()) {
            acc.push(ev, &spec.column, budget)?;
        }
        return Ok(());
    }
    budget.group(&key, specs.len())?;
    order.push(key.clone());
    let mut accs: Vec<Acc> = specs.iter().map(|s| Acc::new(&s.func)).collect();
    for (acc, spec) in accs.iter_mut().zip(specs.iter()) {
        acc.push(ev, &spec.column, budget)?;
    }
    groups.insert(key, accs);
    Ok(())
}

/// Colunas disponíveis direto dos metadados (sem parse da linha).
pub(crate) fn is_meta_column(col: &str) -> bool {
    matches!(col, "*" | "timestamp" | "level")
}

/// Evento parcial montado só com os campos dos metadados.
fn meta_event(idx: &FileIndex, i: usize) -> Event {
    let m = &idx.lines.at(i);
    let mut ev = Event::empty();
    ev.id = i;
    if m.ts != 0 {
        ev.timestamp = Some(m.ts);
    }
    ev.level = crate::model::class_label(m.level).to_string();
    let code = m.code(line_bytes(idx, i));
    if !code.is_empty() {
        ev.code = String::from_utf8_lossy(code).into_owned();
    }
    ev
}

pub fn aggregate_indexed(
    idx: &FileIndex,
    filters: &[Filter],
    group_column: &str,
    specs: &[AggSpec],
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> AggResult {
    match crate::engine::aggregate(&engine_source(idx, codes, system, derived), &prepare(filters), group_column, specs) {
        Ok(Some(result)) => return result,
        Err(error) => return AggResult::failure(error),
        Ok(None) => (),
    }
    let mut budget = AnalyticsBudget::new();
    let mut groups: HashMap<Option<String>, Vec<Acc>> = HashMap::new();
    let mut order: Vec<Option<String>> = Vec::new();
    let meta_only = is_meta_column(group_column) && specs.iter().all(|s| is_meta_column(&s.column));
    let mut failure = None;
    let exceeded = std::sync::atomic::AtomicBool::new(false);
    let scan = scan_indexed(idx, &prepare(filters), codes, system, derived, |i, _, event| {
        if exceeded.load(std::sync::atomic::Ordering::Relaxed) { return None; }
        Some(if meta_only { meta_event(idx, i) } else { event.unwrap_or_else(|| event_at(idx, i, codes, system, derived)) })
    }, |event| {
        if exceeded.load(std::sync::atomic::Ordering::Relaxed) { return; }
        if let Some(event) = event {
            if let Err(error) = push_group(&mut groups, &mut order, &event, event.col_str(group_column), specs, &mut budget) { failure = Some(error); exceeded.store(true, std::sync::atomic::Ordering::Relaxed); }
            if groups.len() > group_budget(specs) { exceeded.store(true, std::sync::atomic::Ordering::Relaxed); }
        }
    });
    if let Err(error) = scan { return AggResult::failure(error); }
    if let Some(error) = failure { return AggResult::failure(error); }
    if exceeded.load(std::sync::atomic::Ordering::Relaxed) { return AggResult::budget_error(); }
    build_agg_result(groups, order, group_column, specs)
}

// ==========================================================================
// Estatísticas (histograma + níveis)
// ==========================================================================

pub fn explore_indexed(
    idx: &FileIndex,
    filters: &[Filter],
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> Result<ExplorerSnapshot, String> {
    if let Some(snapshot) = crate::engine::explore(
        &engine_source(idx, codes, system, derived),
        &prepare(filters),
        sort_column,
        sort_dir,
        offset,
        limit,
    )? {
        return Ok(snapshot);
    }
    let query = query_indexed(idx, filters, sort_column, sort_dir, offset, limit, codes, system, derived)?;
    let stats = stats_indexed(idx, filters, codes, system, derived)?;
    let count = [AggSpec { func: "count".into(), column: "*".into(), alias: "n".into() }];
    let sources = aggregate_indexed(idx, filters, "source", &count, codes, system, derived);
    let codes_agg = aggregate_indexed(idx, filters, "code", &count, codes, system, derived);
    crate::operations::check()?;
    Ok(ExplorerSnapshot { query, stats, sources, codes: codes_agg })
}

pub(crate) fn build_stats(buckets: Vec<(i64, i64)>, bucket_ms: i64, levels: Vec<(String, i64)>) -> Stats {
    Stats {
        buckets,
        bucket_ms,
        levels,
    }
}

const N_BUCKETS: usize = 60;

/// Width and number of histogram buckets covering `[min_ts, max_ts]`.
pub(crate) fn stats_layout(min_ts: i64, max_ts: i64) -> (i64, usize) {
    let span = max_ts.saturating_sub(min_ts).saturating_add(1);
    let bucket_ms = (span / N_BUCKETS as i64)
        .saturating_add(i64::from(span % N_BUCKETS as i64 != 0))
        .max(1);
    let count = (((span - 1) / bucket_ms + 1) as usize).min(N_BUCKETS);
    (bucket_ms, count)
}

/// Level counts, most frequent first; ties by label keep the order stable.
pub(crate) fn sort_levels(levels: &mut [(String, i64)]) {
    levels.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
}

fn stats_from<'a>(iter: impl Iterator<Item = (Option<i64>, &'a str)>) -> Stats {
    // Chaveia por &str emprestada (zero alocação por linha); converte para
    // String apenas no Vec final.
    let mut levels: HashMap<&'a str, i64> = HashMap::new();
    let mut min_ts = i64::MAX;
    let mut max_ts = i64::MIN;
    let mut items = Vec::new();
    for (ts, level) in iter {
        *levels.entry(level).or_default() += 1;
        if let Some(ts) = ts {
            min_ts = min_ts.min(ts);
            max_ts = max_ts.max(ts);
            items.push(ts);
        }
    }

    let mut buckets = Vec::new();
    let mut bucket_ms = 0i64;
    if min_ts <= max_ts {
        let count;
        (bucket_ms, count) = stats_layout(min_ts, max_ts);
        let mut counts = vec![0i64; count];
        for t in items {
            let b = (t.saturating_sub(min_ts) / bucket_ms) as usize;
            counts[b.min(count - 1)] += 1;
        }
        buckets = counts
            .into_iter()
            .enumerate()
            .map(|(b, c)| {
                (
                    min_ts.saturating_add((b as i64).saturating_mul(bucket_ms)),
                    c,
                )
            })
            .collect();
    }

    let mut levels: Vec<(String, i64)> = levels
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    sort_levels(&mut levels);
    build_stats(buckets, bucket_ms, levels)
}

pub fn stats(events: &[Event], filters: &[Filter]) -> Stats {
    let idx = filtered_indices(events, filters);
    stats_from(idx.into_iter().map(|i| {
        let ev = &events[i];
        (ev.timestamp, ev.level.as_str())
    }))
}

pub fn stats_indexed(
    idx: &FileIndex,
    filters: &[Filter],
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> Result<Stats, String> {
    let pfs = prepare(filters);
    if let Some(stats) = crate::engine::stats(&engine_source(idx, codes, system, derived), &pfs)? {
        return Ok(stats);
    }
    // Two bounded passes trade recovery CPU for predictable memory. No vector
    // of timestamps/matches proportional to a 50M-row source is retained.
    let (mut min_ts, mut max_ts) = (i64::MAX, i64::MIN);
    let mut levels: HashMap<&'static str, i64> = HashMap::new();
    scan_indexed(
        idx, &pfs, codes, system, derived,
        |_, meta, _| (meta.ts, crate::model::class_label(meta.level)),
        |(ts, level)| {
            *levels.entry(level).or_default() += 1;
            if ts != 0 { min_ts = min_ts.min(ts); max_ts = max_ts.max(ts); }
        },
    )?;
    let (mut buckets, mut bucket_ms) = (Vec::new(), 0);
    if min_ts <= max_ts && !crate::operations::cancelled() {
        let count;
        (bucket_ms, count) = stats_layout(min_ts, max_ts);
        let mut counts = vec![0i64; count];
        scan_indexed(idx, &pfs, codes, system, derived, |_, meta, _| meta.ts, |ts| {
            if ts != 0 {
                let bucket = (ts.saturating_sub(min_ts) / bucket_ms) as usize;
                counts[bucket.min(count - 1)] += 1;
            }
        })?;
        buckets = counts.into_iter().enumerate().map(|(i, n)| (min_ts.saturating_add((i as i64).saturating_mul(bucket_ms)), n)).collect();
    }
    let mut levels: Vec<_> = levels.into_iter().map(|(key, n)| (key.to_string(), n)).collect();
    sort_levels(&mut levels);
    crate::operations::check()?;
    Ok(build_stats(buckets, bucket_ms, levels))
}

#[cfg(test)]
mod projected_page_parity_tests {
    use super::*;
    use crate::page_projection::{ProjectionPlan, ProjectionRequest, Receipt};

    fn plan(columns: &[&str], limit: usize) -> ProjectionPlan {
        ProjectionPlan::new(ProjectionRequest {
            projection_version: 1, columns: columns.iter().map(|s| s.to_string()).collect(),
            cell_bytes: None, response_bytes: None,
        }, limit, Receipt {
            analysis_context: crate::analysis_context::Identity { case_id: "test".into(), analysis_id: "test-analysis".into(), config_revision: 1, visibility_revision: 0 },
            source_generation: Some(1), case_key: None,
            case_content_token: None,
            catalog_signature: crate::engine::catalog_content_signature(&CodesConfig::default(), &CodesConfig::default()),
            catalog_epoch: crate::engine::catalog_token(&CodesConfig::default(), &CodesConfig::default()).epoch,
        }).unwrap()
    }

    #[test]
    fn projected_memory_uses_full_page_filter_order_and_count() {
        let events: Vec<_> = [Some(-1), None, Some(0), Some(1)].into_iter().enumerate().map(|(i, timestamp)| {
            let mut event = Event::empty();
            event.id = 10 + i;
            event.event_ref = format!("memory:{i}");
            event.timestamp = timestamp;
            event.fields.insert("value".into(), serde_json::json!((["10", "2", "11x", "1"][i])));
            event.fields.insert("structured".into(), serde_json::json!({"n": i, "null": null}));
            event
        }).collect();
        let filters = [Filter { column: "id".into(), op: "gte".into(), value: "11".into(), value2: None }];
        for column in ["id", "timestamp", "value"] {
            for direction in ["asc", "desc"] {
                for offset in [0, 1, 4] {
                    let full = query(&events, &filters, column, direction, offset, 2);
                    let projected = query_projected_memory(&events, &filters, column, direction, offset, &plan(&["timestamp", "structured"], 2)).unwrap();
                    assert_eq!(projected.items.iter().map(|r| r.row.id).collect::<Vec<_>>(), full.rows.iter().map(|e| e.id).collect::<Vec<_>>());
                    assert_eq!(projected.total, Some(full.total));
                    assert_eq!(projected.has_more, offset + full.rows.len() < full.total);
                    assert!(projected.next_cursor.is_none());
                    for (item, event) in projected.items.iter().zip(&full.rows) {
                        assert_eq!(item.row.event_ref, event.event_ref);
                        assert_eq!(serde_json::to_value(&item.cells[1]).unwrap()["value"], event.fields["structured"]);
                    }
                }
            }
        }
    }

    #[test]
    fn projected_line_selection_preserves_order_and_wide_row_membership() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("projected-lines.jsonl");
        let raw = [serde_json::json!({"message":"first", "timestamp":"1969-12-31T23:59:59.999Z", "body":"x".repeat(64 << 10)}),
            serde_json::json!({"message":"second", "timestamp": null, "body": {"nested": true}}),
            serde_json::json!({"message":"third", "timestamp":"1970-01-01T00:00:00Z", "body": ""})];
        std::fs::write(&path, raw.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")).unwrap();
        let index = crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let config = CodesConfig::default();
        for direction in ["asc", "desc"] {
            let full = query_page_lines(&index, &[], "timestamp", direction, 0, 2, &config, &config, &[]).unwrap();
            let selected = select_page_lines(&index, &[], "timestamp", direction, 0, 2, &config, &config, &[]).unwrap();
            let plan = plan(&["message", "body"], 2);
            let items = selected.ids.iter().map(|&id| crate::page_projection::project_indexed_row(&index, id, &config, &config, &[], &plan).unwrap()).collect();
            let projected = plan.finish(selected, items).unwrap();
            assert_eq!(projected.items.iter().map(|r| r.row.id).collect::<Vec<_>>(), full.rows.iter().map(|e| e.id).collect::<Vec<_>>());
            assert_eq!(projected.total, full.total);
            assert_eq!(projected.has_more, full.has_more);
            assert_eq!(projected.next_cursor, full.next_cursor);
            assert_eq!(projected.engine, full.engine);
            assert_eq!(projected.warning, full.warning);
        }
        // Membership is known from metadata, so an over-wide original can
        // retain its exact ordinary source handle without source hydration.
        let wide_path = directory.path().join("too-wide.jsonl");
        std::fs::write(&wide_path, serde_json::json!({"message":"wide", "body":"x".repeat((2 << 20) + 1)}).to_string()).unwrap();
        let wide = crate::sources::index_file(wide_path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let plan = plan(&["message"], 1);
        let projected = crate::page_projection::project_indexed_row(&wide, 0, &config, &config, &[], &plan).unwrap();
        assert_eq!(projected.row.event_ref, event_at(&wide, 0, &config, &config, &[]).event_ref);
        assert!(matches!(projected.cells[0], crate::page_projection::Cell::Unavailable { reason: "record_too_wide" }));
    }
}

#[cfg(test)]
mod regex_program_reuse_tests {
    use super::*;
    use crate::query_regex::testing;

    fn filter(op: &str, value: &str) -> Filter {
        Filter { column: "message".into(), op: op.into(), value: value.into(), value2: None }
    }

    #[test]
    fn invalid_regex_errors_and_infallible_prepare_fallback_remain_unchanged() {
        let (_, builds) = testing::run(96 << 20, || {
            let filters = [filter("regex", "[")];
            let error = regex::Regex::new("[").unwrap_err();
            for _ in 0..2 {
                assert_eq!(crate::workspace::validate(&filters).unwrap_err(), format!("Expressão inválida: {error}"));
                let pfs = prepare(&filters);
                assert!(pfs[0].regex.is_none());
                assert!(!value_matches(&pfs[0], Some("anything")));
            }
        });
        assert_eq!(builds, 4, "invalid programs are not cached");
    }

    #[test]
    fn cancellation_between_regex_and_list_compilations_is_explicit() {
        for text in ["message:/first/ AND message:/second/", "path:(first* OR second* OR third*)"] {
            let (_, builds) = testing::run(96 << 20, || {
                let id = format!("regex-cancel-{}", uuid::Uuid::new_v4());
                let token = crate::operations::token(Some(id.clone())).unwrap();
                let result = testing::with_after_build(move || { crate::operations::cancel_id(&id); }, || {
                    crate::operations::run_with_token(token, || {
                        assert_eq!(crate::querylang::compile(text).unwrap_err(), "Operação cancelada.");
                    })
                });
                assert_eq!(result.unwrap_err(), "Operação cancelada.");
                // The cancelled first program was never published. A retry must
                // compile all programs, including the first, then can reuse them.
                crate::querylang::compile(text).unwrap();
                crate::querylang::compile(text).unwrap();
            });
            assert_eq!(builds, if text.starts_with("path:") { 4 } else { 3 });
        }
    }
}
